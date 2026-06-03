import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export type AuthConfig =
	| { mode: "auto" }
	| { mode: "none" }
	| {
			mode: "bearer";
			token?: string;
			tokenFile?: string;
			providerToken?: string;
			providerTokenFile?: string;
			userToken?: string;
			userTokenFile?: string;
			userId?: string;
			providerUserId?: string;
			userUserId?: string;
	  };

export type LedgerRole = "single" | "provider" | "user";

export type SandboxScenario = {
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

export type LocalNetTwoParticipantsScenario = {
	scenario: "localnet-two-participants";
	providerLedger: string;
	userLedger: string;
	plaintext: boolean;
	auth: AuthConfig;
	uploadDar: boolean;
	parties: {
		operatorHint: string;
		counterpartyHint: string;
	};
	submitLedgerRole: "provider";
	exerciseLedgerRole: "user";
};

export type Scenario = SandboxScenario | LocalNetTwoParticipantsScenario;

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

function assertNever(value: never): never {
	throw new Error(`Unhandled variant: ${JSON.stringify(value)}`);
}

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

async function runText(
	command: string[],
	options: RunOptions = {},
): Promise<string> {
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

async function runJson<T>(
	command: string[],
	options: RunOptions = {},
): Promise<T> {
	const stdout = await runText(command, options);
	const jsonStart = stdout.search(/[[{]/);
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

async function readTokenFile(path: string | undefined): Promise<string | undefined> {
	if (!path) {
		return undefined;
	}

	return (await Bun.file(path).text()).trim();
}

async function bearerToken(auth: AuthConfig, role: LedgerRole): Promise<string | undefined> {
	switch (auth.mode) {
		case "auto":
		case "none":
			return undefined;
		case "bearer":
			switch (role) {
				case "provider":
					return (
						auth.providerToken ??
						(await readTokenFile(auth.providerTokenFile)) ??
						process.env.LEDGER_API_PROVIDER_TOKEN ??
						auth.token ??
						(await readTokenFile(auth.tokenFile)) ??
						process.env.LEDGER_API_TOKEN
					);
				case "user":
					return (
						auth.userToken ??
						(await readTokenFile(auth.userTokenFile)) ??
						process.env.LEDGER_API_USER_TOKEN ??
						auth.token ??
						(await readTokenFile(auth.tokenFile)) ??
						process.env.LEDGER_API_TOKEN
					);
				case "single":
					return (
						auth.token ??
						(await readTokenFile(auth.tokenFile)) ??
						process.env.LEDGER_API_TOKEN
					);
			}

			return assertNever(role);
	}

	return assertNever(auth);
}

async function ledgerRequest<T>(
	scenario: Scenario,
	role: LedgerRole,
	service: string,
	data: unknown,
): Promise<T> {
	const args = ["grpcurl"];
	if (scenario.plaintext) {
		args.push("-plaintext");
	}

	const token = await bearerToken(scenario.auth, role);
	if (token) {
		args.push("-H", `authorization: Bearer ${token}`);
	}

	args.push("-d", JSON.stringify(data), ledgerForRole(scenario, role), service);
	try {
		return await runJson<T>(args);
	} catch (error) {
		if (
			error instanceof CommandError &&
			scenario.auth.mode === "none" &&
			/Unauthenticated/i.test(`${error.stdout}\n${error.stderr}`)
		) {
			throw new Error(
				[
					"Ledger API rejected the request as unauthenticated.",
					"Use auth.mode=\"auto\" for LocalNet token generation, or provide bearer tokens in scenario.local.json.",
					`Original command: ${error.command.join(" ")}`,
					error.stderr.trim(),
				].join("\n"),
			);
		}

		throw error;
	}
}

export function ledgerForRole(scenario: Scenario, role: LedgerRole): string {
	switch (scenario.scenario) {
		case "sandbox":
			switch (role) {
				case "single":
					return scenario.ledger;
				case "provider":
				case "user":
					throw new Error(`Sandbox scenario does not have a ${role} ledger`);
			}

			return assertNever(role);
		case "localnet-two-participants":
			switch (role) {
				case "provider":
					return scenario.providerLedger;
				case "user":
					return scenario.userLedger;
				case "single":
					throw new Error(
						"LocalNet two-participant scenario requires provider or user ledger role",
					);
			}

			return assertNever(role);
	}

	return assertNever(scenario);
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
	return ledgerApiVersionForRole(scenario, "single");
}

export async function ledgerApiVersionForRole(
	scenario: Scenario,
	role: LedgerRole,
): Promise<unknown> {
	return ledgerRequest(
		scenario,
		role,
		"com.daml.ledger.api.v2.VersionService/GetLedgerApiVersion",
		{},
	);
}

export async function ledgerEnd(scenario: Scenario): Promise<unknown> {
	return ledgerEndForRole(scenario, "single");
}

export async function ledgerEndForRole(
	scenario: Scenario,
	role: LedgerRole,
): Promise<unknown> {
	return ledgerRequest(
		scenario,
		role,
		"com.daml.ledger.api.v2.StateService/GetLedgerEnd",
		{},
	);
}

export async function uploadDar(
	scenario: Scenario,
	role: LedgerRole,
	darPath: string,
): Promise<unknown> {
	const bytes = Buffer.from(await Bun.file(darPath).arrayBuffer()).toString(
		"base64",
	);

	try {
		return await ledgerRequest(
			scenario,
			role,
			"com.daml.ledger.api.v2.admin.PackageManagementService/UploadDarFile",
			{ darFile: bytes },
		);
	} catch (error) {
		if (error instanceof CommandError) {
			const output = `${error.stdout}\n${error.stderr}`;
			if (/already exists|ALREADY_EXISTS|PACKAGE.*EXISTS/i.test(output)) {
				return {
					skipped: true,
					reason: "DAR already uploaded",
					ledger: ledgerForRole(scenario, role),
				};
			}
		}

		throw error;
	}
}

export async function allocateParty(
	scenario: Scenario,
	role: LedgerRole,
	partyIdHint: string,
): Promise<string> {
	const result = await ledgerRequest<{
		partyDetails?: { party?: string };
		party_details?: { party?: string };
	}>(
		scenario,
		role,
		"com.daml.ledger.api.v2.admin.PartyManagementService/AllocateParty",
		{
			partyIdHint,
		},
	);

	const party = result.partyDetails?.party ?? result.party_details?.party;
	if (!party) {
		throw new Error(
			`AllocateParty did not return a party for hint ${partyIdHint}`,
		);
	}

	return party;
}

function isAlreadyExistsError(error: unknown): boolean {
	if (!(error instanceof CommandError)) {
		return false;
	}

	return /already exists|ALREADY_EXISTS|AlreadyExists/i.test(
		`${error.stdout}\n${error.stderr}`,
	);
}

function partyRights(party: string): Array<Record<string, unknown>> {
	return [{ canActAs: { party } }, { canReadAs: { party } }];
}

export async function ensureUserWithPartyRights(
	scenario: Scenario,
	role: LedgerRole,
	userId: string,
	party: string,
): Promise<unknown> {
	try {
		return await ledgerRequest(
			scenario,
			role,
			"com.daml.ledger.api.v2.admin.UserManagementService/CreateUser",
			{
				user: {
					id: userId,
					primaryParty: party,
				},
				rights: partyRights(party),
			},
		);
	} catch (error) {
		if (!isAlreadyExistsError(error)) {
			throw error;
		}
	}

	try {
		return await ledgerRequest(
			scenario,
			role,
			"com.daml.ledger.api.v2.admin.UserManagementService/GrantUserRights",
			{
				userId,
				rights: partyRights(party),
			},
		);
	} catch (error) {
		if (isAlreadyExistsError(error)) {
			return {
				skipped: true,
				reason: "User already had required rights",
				userId,
				party,
			};
		}

		throw error;
	}
}

export async function submitCreate(
	scenario: Scenario,
	role: LedgerRole,
	packageId: string,
	operator: string,
	counterparty: string,
	parties: string[] = [operator, counterparty],
	userId = "trace-probe",
): Promise<unknown> {
	const commandId = `trace-probe-create-${Date.now()}`;
	const format = updateFormat(parties);

	return ledgerRequest(
		scenario,
		role,
		"com.daml.ledger.api.v2.CommandService/SubmitAndWaitForTransaction",
		{
			commands: {
			userId,
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
		},
	);
}

export async function submitAccept(
	scenario: Scenario,
	role: LedgerRole,
	packageId: string,
	counterparty: string,
	contractId: string,
	userId = "trace-probe",
): Promise<unknown> {
	const commandId = `trace-probe-accept-${Date.now()}`;
	const format = updateFormat([counterparty]);

	return ledgerRequest(
		scenario,
		role,
		"com.daml.ledger.api.v2.CommandService/SubmitAndWaitForTransaction",
		{
			commands: {
			userId,
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
									fields: [
										{
											label: "note",
											value: { text: "accepted for sandbox trace" },
										},
									],
								},
							},
						},
					},
				],
			},
			transactionFormat: format.includeTransactions,
		},
	);
}

export async function getUpdateById(
	scenario: Scenario,
	role: LedgerRole,
	updateId: string,
	parties: string[],
): Promise<unknown> {
	return ledgerRequest(
		scenario,
		role,
		"com.daml.ledger.api.v2.UpdateService/GetUpdateById",
		{
			updateId,
			updateFormat: updateFormat(parties),
		},
	);
}

export async function connectedSynchronizers(
	scenario: Scenario,
	role: LedgerRole,
	party: string,
): Promise<unknown> {
	return ledgerRequest(
		scenario,
		role,
		"com.daml.ledger.api.v2.StateService/GetConnectedSynchronizers",
		{
			party,
		},
	);
}
