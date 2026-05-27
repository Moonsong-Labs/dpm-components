import { existsSync } from "node:fs";
import { join } from "node:path";
import {
  allocateParty,
  buildProbe,
  getUpdateById,
  ledgerApiVersion,
  ledgerEnd,
  mainPackageId,
  projectRoot,
  submitAccept,
  submitCreate,
  type Scenario,
} from "./ledger-api";

type TransactionResponse = {
  transaction?: {
    updateId?: string;
    update_id?: string;
    synchronizerId?: string;
    synchronizer_id?: string;
    events?: Array<Record<string, unknown>>;
  };
};

async function readJson<T>(path: string): Promise<T> {
  return (await Bun.file(path).json()) as T;
}

async function writeJson(path: string, value: unknown): Promise<void> {
  await Bun.write(path, `${JSON.stringify(value, null, 2)}\n`);
}

function mergeScenario(base: Scenario, override: Partial<Scenario>): Scenario {
  return {
    ...base,
    ...override,
    auth: {
      ...base.auth,
      ...override.auth,
    } as Scenario["auth"],
    parties: {
      ...base.parties,
      ...override.parties,
    },
  };
}

async function loadScenario(name: string): Promise<Scenario> {
  const scenarioPath = join(projectRoot, "scenarios", `${name}.json`);
  const overridePath = join(projectRoot, "fixtures", name, "scenario.local.json");

  const base = await readJson<Scenario>(scenarioPath);
  // TODO: remove this once we have more scenarios
  if (base.scenario !== "sandbox") {
    throw new Error(`Only the sandbox scenario is implemented, got ${base.scenario}`);
  }

  if (!existsSync(overridePath)) {
    return base;
  }

  return mergeScenario(base, await readJson<Partial<Scenario>>(overridePath));
}

function transaction(response: unknown): NonNullable<TransactionResponse["transaction"]> {
  const tx = (response as TransactionResponse).transaction;
  if (!tx) {
    throw new Error("Ledger API response did not contain a transaction");
  }

  return tx;
}

function updateId(response: unknown): string {
  const tx = transaction(response);
  const id = tx.updateId ?? tx.update_id;
  if (!id) {
    throw new Error("Transaction response did not contain updateId");
  }

  return id;
}

function createdContractId(response: unknown): string {
  for (const event of transaction(response).events ?? []) {
    const created = event.created as { contractId?: string; contract_id?: string } | undefined;
    const contractId = created?.contractId ?? created?.contract_id;
    if (contractId) {
      return contractId;
    }
  }

  throw new Error("Create response did not contain a created contract id");
}

function eventKinds(response: unknown): string[] {
  return [
    ...new Set(
      (transaction(response).events ?? []).flatMap((event) =>
        ["created", "exercised", "archived"].filter((kind) => event[kind] !== undefined),
      ),
    ),
  ];
}

async function captureSandbox(): Promise<void> {
  const scenario = await loadScenario("sandbox");
  const fixtureDir = join(projectRoot, "fixtures", "sandbox");

  await Bun.$`mkdir -p ${fixtureDir}`;

  const darPath = await buildProbe();
  const packageId = await mainPackageId(darPath);

  const version = await ledgerApiVersion(scenario);
  await writeJson(join(fixtureDir, "ledger-api-version.json"), version);

  const before = await ledgerEnd(scenario);
  await writeJson(join(fixtureDir, "ledger-end-before.json"), before);

  const runId = Date.now();
  const operator = await allocateParty(scenario, `${scenario.parties.operatorHint}${runId}`);
  const counterparty = await allocateParty(scenario, `${scenario.parties.counterpartyHint}${runId}`);
  await writeJson(join(fixtureDir, "parties.json"), { operator, counterparty });

  const createSubmit = await submitCreate(scenario, packageId, operator, counterparty);
  await writeJson(join(fixtureDir, "create-submit.json"), createSubmit);

  const createUpdateId = updateId(createSubmit);
  const probeContractId = createdContractId(createSubmit);
  const createUpdate = await getUpdateById(scenario, createUpdateId, [operator, counterparty]);
  await writeJson(join(fixtureDir, "create-update.json"), createUpdate);

  const exerciseSubmit = await submitAccept(scenario, packageId, counterparty, probeContractId);
  await writeJson(join(fixtureDir, "exercise-submit.json"), exerciseSubmit);

  const exerciseUpdateId = updateId(exerciseSubmit);
  const exerciseUpdate = await getUpdateById(scenario, exerciseUpdateId, [operator, counterparty]);
  await writeJson(join(fixtureDir, "exercise-update.json"), exerciseUpdate);

  const summary = {
    scenario: scenario.scenario,
    ledger: scenario.ledger,
    tls: !scenario.plaintext,
    auth: scenario.auth.mode,
    operator,
    counterparty,
    createUpdateId,
    exerciseUpdateId,
    createSynchronizerId:
      transaction(createUpdate).synchronizerId ?? transaction(createUpdate).synchronizer_id,
    exerciseSynchronizerId:
      transaction(exerciseUpdate).synchronizerId ?? transaction(exerciseUpdate).synchronizer_id,
    createEventKinds: eventKinds(createUpdate),
    exerciseEventKinds: eventKinds(exerciseUpdate),
  };

  await writeJson(join(fixtureDir, "summary.json"), summary);
  console.log(JSON.stringify(summary, null, 2));
}

async function main(): Promise<void> {
  const scenarioName = process.argv[2];
  if (scenarioName !== "sandbox") {
    console.error("Usage: bun scripts/capture.ts sandbox");
    process.exit(1);
  }

  await captureSandbox();
}

await main();
