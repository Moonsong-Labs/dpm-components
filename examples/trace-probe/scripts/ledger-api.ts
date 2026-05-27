import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export type AuthConfig =
  | { mode: "none" }
  | { mode: "bearer"; token?: string; tokenFile?: string };

export type Scenario = {
  scenario: "sandbox";
  ledger: string;
  plaintext: boolean;
  auth: AuthConfig;
  uploadDar: boolean;
  parties: {
    operatorHint: string;
    counterpartyHint: string;
  };
  submitLedgerRole: "single";
};

export type UpdateFormat = {
  includeTransactions: {
    eventFormat: {
      filtersByParty: Record<
        string,
        {
          cumulative: Array<{
            wildcardFilter: {
              includeCreatedEventBlob: boolean;
            };
          }>;
        }
      >;
      verbose: boolean;
    };
    transactionShape: "TRANSACTION_SHAPE_LEDGER_EFFECTS";
  };
};

const scriptsDir = dirname(fileURLToPath(import.meta.url));
export const projectRoot = join(scriptsDir, "..");
export const probeDarPath = join(
  projectRoot,
  "main",
  ".daml",
  "dist",
  "trace-probe-main-1.0.0.dar",
);

type RunOptions = {
  cwd?: string;
  allowFailure?: boolean;
};

export class CommandError extends Error {
  constructor(
    message: string,
    readonly command: string[],
    readonly exitCode: number,
    readonly stdout: string,
    readonly stderr: string,
  ) {
    super(message);
  }
}

async function runText(command: string[], options: RunOptions = {}): Promise<string> {
  const proc = Bun.spawn(command, {
    cwd: options.cwd ?? projectRoot,
    stdout: "pipe",
    stderr: "pipe",
  });

  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);

  if (exitCode !== 0 && !options.allowFailure) {
    throw new CommandError(
      `Command failed with exit code ${exitCode}: ${command.join(" ")}`,
      command,
      exitCode,
      stdout,
      stderr,
    );
  }

  return stdout;
}

async function runJson<T>(command: string[], options: RunOptions = {}): Promise<T> {
  const stdout = await runText(command, options);
  const jsonStart = stdout.search(/[\[{]/);
  if (jsonStart < 0) {
    throw new Error(`Command did not return JSON: ${command.join(" ")}`);
  }

  return JSON.parse(stdout.slice(jsonStart)) as T;
}

export async function buildProbe(): Promise<string> {
  await runText(["dpm", "build", "--all"]);

  if (!existsSync(probeDarPath)) {
    throw new Error(`Expected probe DAR was not created at ${probeDarPath}`);
  }

  return probeDarPath;
}

export async function mainPackageId(darPath = probeDarPath): Promise<string> {
  const inspectors = [
    ["dpm", "inspect-dar", "--json", darPath],
    ["daml", "damlc", "inspect-dar", "--json", darPath],
  ];

  for (const inspector of inspectors) {
    try {
      const result = await runJson<Record<string, unknown>>(inspector);
      const packageId = result.main_package_id ?? result.mainPackageId;
      if (typeof packageId === "string" && packageId.length > 0) {
        return packageId;
      }
    } catch {
      // Try the next installed tool; SDK command names differ across versions.
    }
  }

  throw new Error(`Could not inspect main package id for ${darPath}`);
}

async function bearerToken(auth: AuthConfig): Promise<string | undefined> {
  if (auth.mode !== "bearer") {
    return undefined;
  }

  if (auth.token) {
    return auth.token;
  }

  if (auth.tokenFile) {
    return (await Bun.file(auth.tokenFile).text()).trim();
  }

  return process.env.LEDGER_API_TOKEN;
}

async function ledgerRequest<T>(
  scenario: Pick<Scenario, "ledger" | "plaintext" | "auth">,
  service: string,
  data: unknown,
): Promise<T> {
  const args = ["grpcurl"];
  if (scenario.plaintext) {
    args.push("-plaintext");
  }

  const token = await bearerToken(scenario.auth);
  if (token) {
    args.push("-H", `authorization: Bearer ${token}`);
  }

  args.push("-d", JSON.stringify(data), scenario.ledger, service);
  return runJson<T>(args);
}

export function updateFormat(parties: string[]): UpdateFormat {
  return {
    includeTransactions: {
      eventFormat: {
        filtersByParty: Object.fromEntries(
          parties.map((party) => [
            party,
            {
              cumulative: [
                {
                  wildcardFilter: {
                    includeCreatedEventBlob: true,
                  },
                },
              ],
            },
          ]),
        ),
        verbose: true,
      },
      transactionShape: "TRANSACTION_SHAPE_LEDGER_EFFECTS",
    },
  };
}

export async function ledgerApiVersion(scenario: Scenario): Promise<unknown> {
  return ledgerRequest(scenario, "com.daml.ledger.api.v2.VersionService/GetLedgerApiVersion", {});
}

export async function ledgerEnd(scenario: Scenario): Promise<unknown> {
  return ledgerRequest(scenario, "com.daml.ledger.api.v2.StateService/GetLedgerEnd", {});
}

export async function allocateParty(
  scenario: Scenario,
  partyIdHint: string,
): Promise<string> {
  const result = await ledgerRequest<{
    partyDetails?: { party?: string };
    party_details?: { party?: string };
  }>(
    scenario,
    "com.daml.ledger.api.v2.admin.PartyManagementService/AllocateParty",
    { partyIdHint },
  );

  const party = result.partyDetails?.party ?? result.party_details?.party;
  if (!party) {
    throw new Error(`AllocateParty did not return a party for hint ${partyIdHint}`);
  }

  return party;
}

export async function submitCreate(
  scenario: Scenario,
  packageId: string,
  operator: string,
  counterparty: string,
): Promise<unknown> {
  const commandId = `trace-probe-create-${Date.now()}`;
  const format = updateFormat([operator, counterparty]);

  return ledgerRequest(scenario, "com.daml.ledger.api.v2.CommandService/SubmitAndWaitForTransaction", {
    commands: {
      userId: "trace-probe",
      commandId,
      actAs: [operator],
      commands: [
        {
          create: {
            templateId: {
              packageId,
              moduleName: "TraceProbe",
              entityName: "Probe",
            },
            createArguments: {
              fields: [
                { label: "operator", value: { party: operator } },
                { label: "counterparty", value: { party: counterparty } },
                { label: "label", value: { text: "sandbox trace probe" } },
              ],
            },
          },
        },
      ],
    },
    transactionFormat: format.includeTransactions,
  });
}

export async function submitAccept(
  scenario: Scenario,
  packageId: string,
  counterparty: string,
  contractId: string,
): Promise<unknown> {
  const commandId = `trace-probe-accept-${Date.now()}`;
  const format = updateFormat([counterparty]);

  return ledgerRequest(scenario, "com.daml.ledger.api.v2.CommandService/SubmitAndWaitForTransaction", {
    commands: {
      userId: "trace-probe",
      commandId,
      actAs: [counterparty],
      commands: [
        {
          exercise: {
            templateId: {
              packageId,
              moduleName: "TraceProbe",
              entityName: "Probe",
            },
            contractId,
            choice: "Accept",
            choiceArgument: {
              record: {
                fields: [{ label: "note", value: { text: "accepted for sandbox trace" } }],
              },
            },
          },
        },
      ],
    },
    transactionFormat: format.includeTransactions,
  });
}

export async function getUpdateById(
  scenario: Scenario,
  updateId: string,
  parties: string[],
): Promise<unknown> {
  return ledgerRequest(scenario, "com.daml.ledger.api.v2.UpdateService/GetUpdateById", {
    updateId,
    updateFormat: updateFormat(parties),
  });
}
