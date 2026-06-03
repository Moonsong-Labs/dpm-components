import { existsSync } from "node:fs";
import { join } from "node:path";
import {
	allocateParty,
	buildProbe,
	CommandError,
	connectedSynchronizers,
	ensureUserWithPartyRights,
	getUpdateById,
	type LocalNetTwoParticipantsScenario,
	ledgerApiVersion,
	ledgerApiVersionForRole,
	ledgerEnd,
	ledgerEndForRole,
	ledgerForRole,
	mainPackageId,
	projectRoot,
	type Scenario,
	submitAccept,
	submitCreate,
	uploadDar,
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

async function runText(command: string[]): Promise<string> {
	const proc = Bun.spawn(command, {
		stdout: "pipe",
		stderr: "pipe",
	});

	const [stdout, stderr, exitCode] = await Promise.all([
		new Response(proc.stdout).text(),
		new Response(proc.stderr).text(),
		proc.exited,
	]);

	if (exitCode !== 0) {
		throw new CommandError(
			`Command failed with exit code ${exitCode}: ${command.join(" ")}`,
			command,
			exitCode,
			stdout,
			stderr,
		);
	}

	return stdout.trim();
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
	} as Scenario;
}

async function loadScenario(name: string): Promise<Scenario> {
	const scenarioPath = join(projectRoot, "scenarios", `${name}.json`);
	const overridePath = join(
		projectRoot,
		"fixtures",
		name,
		"scenario.local.json",
	);

	const base = await readJson<Scenario>(scenarioPath);

	if (!existsSync(overridePath)) {
		return base;
	}

	return mergeScenario(base, await readJson<Partial<Scenario>>(overridePath));
}

function transaction(
	response: unknown,
): NonNullable<TransactionResponse["transaction"]> {
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
		const created = event.created as
			| { contractId?: string; contract_id?: string }
			| undefined;
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
				["created", "exercised", "archived"].filter(
					(kind) => event[kind] !== undefined,
				),
			),
		),
	];
}

function visibleEventKinds(response: unknown | undefined): string[] {
	if (!response) {
		return [];
	}

	try {
		return eventKinds(response);
	} catch {
		return [];
	}
}

function serialiseError(error: unknown): Record<string, unknown> {
	if (error instanceof CommandError) {
		return {
			message: error.message,
			command: error.command,
			exitCode: error.exitCode,
			stdout: error.stdout,
			stderr: error.stderr,
		};
	}

	if (error instanceof Error) {
		return {
			name: error.name,
			message: error.message,
			stack: error.stack,
		};
	}

	return { error };
}

async function sleep(ms: number): Promise<void> {
	await new Promise((resolve) => setTimeout(resolve, ms));
}

async function retry<T>(label: string, action: () => Promise<T>): Promise<T> {
	let lastError: unknown;

	for (let attempt = 1; attempt <= 10; attempt += 1) {
		try {
			return await action();
		} catch (error) {
			lastError = error;
			if (attempt < 10) {
				await sleep(1_000);
			}
		}
	}

	throw new Error(
		`${label} failed after retries: ${JSON.stringify(serialiseError(lastError))}`,
	);
}

async function writeResult(
	fixtureDir: string,
	filename: string,
	action: () => Promise<unknown>,
): Promise<unknown | undefined> {
	try {
		const result = await action();
		await writeJson(join(fixtureDir, filename), result);
		return result;
	} catch (error) {
		await writeJson(
			join(fixtureDir, filename.replace(/\.json$/, "-error.json")),
			serialiseError(error),
		);
		return undefined;
	}
}

type LocalNetAuthMode = "oauth2" | "shared-secret";

// Development-only OAuth2 clients seeded by Canton Network LocalNet.
// Docs: https://docs.canton.network/appdev/quickstart/json-api#user-token-and-ids-cookbook
const localNetOauthClients = {
	provider: {
		realm: "AppProvider",
		clientId: "app-provider-validator",
		clientSecret: "AL8648b9SfdTFImq7FV56Vd0KHifHBuC",
	},
	user: {
		realm: "AppUser",
		clientId: "app-user-validator",
		clientSecret: "6m12QyyGl81d9nABWQXMycZdXho6ejEX",
	},
} as const;

async function localNetAuthMode(): Promise<LocalNetAuthMode> {
	const response = await fetch(
		"http://app-provider.localhost:3000/api/feature-flags",
	);
	if (!response.ok) {
		throw new Error(
			`Could not detect LocalNet auth mode from /api/feature-flags: ${response.status} ${response.statusText}`,
		);
	}

	const body = (await response.json()) as { authMode?: string };
	switch (body.authMode) {
		case "oauth2":
		case "shared-secret":
			return body.authMode;
		default:
			throw new Error(
				`Unsupported LocalNet auth mode: ${JSON.stringify(body)}`,
			);
	}
}

async function keycloakToken(
	realm: "AppProvider" | "AppUser",
	clientId: string,
	clientSecret: string,
): Promise<string> {
	const body = new URLSearchParams({
		client_id: clientId,
		client_secret: clientSecret,
		grant_type: "client_credentials",
		scope: "openid",
	});
	const response = await fetch(
		`http://keycloak.localhost:8082/realms/${realm}/protocol/openid-connect/token`,
		{
			method: "POST",
			headers: { "Content-Type": "application/x-www-form-urlencoded" },
			body,
		},
	);

	if (!response.ok) {
		throw new Error(
			`Could not get ${realm} token from Keycloak: ${response.status} ${response.statusText} ${await response.text()}`,
		);
	}

	const tokenResponse = (await response.json()) as { access_token?: string };
	if (!tokenResponse.access_token) {
		throw new Error(`Keycloak ${realm} response did not include access_token`);
	}

	return tokenResponse.access_token;
}

function base64UrlDecode(value: string): string {
	const base64 = value.replaceAll("-", "+").replaceAll("_", "/");
	const padded = base64.padEnd(
		base64.length + ((4 - (base64.length % 4)) % 4),
		"=",
	);
	return Buffer.from(padded, "base64").toString("utf8");
}

function jwtSubject(token: string): string {
	const payload = token.split(".")[1];
	if (!payload) {
		throw new Error("JWT did not contain a payload segment");
	}

	const claims = JSON.parse(base64UrlDecode(payload)) as { sub?: string };
	if (!claims.sub) {
		throw new Error("JWT did not contain a sub claim");
	}

	return claims.sub;
}

async function sharedSecretToken(subject: string): Promise<string> {
	return runText([
		"docker",
		"exec",
		"splice-onboarding",
		"jwt-cli",
		"encode",
		"hs256",
		"--s",
		"unsafe",
		"--p",
		JSON.stringify({
			sub: subject,
			aud: "https://canton.network.global",
		}),
	]);
}

async function generatedLocalNetAuth(): Promise<{
	authMode: LocalNetAuthMode;
	providerToken: string;
	userToken: string;
	providerUserId: string;
	userUserId: string;
}> {
	const authMode = await localNetAuthMode();

	switch (authMode) {
		case "oauth2": {
			const { provider, user } = localNetOauthClients;
			const providerToken = await keycloakToken(
				provider.realm,
				provider.clientId,
				provider.clientSecret,
			);
			const userToken = await keycloakToken(
				user.realm,
				user.clientId,
				user.clientSecret,
			);

			return {
				authMode,
				providerToken,
				userToken,
				providerUserId: jwtSubject(providerToken),
				userUserId: jwtSubject(userToken),
			};
		}
		case "shared-secret": {
			const token = await sharedSecretToken("ledger-api-user");
			return {
				authMode,
				providerToken: token,
				userToken: token,
				providerUserId: "ledger-api-user",
				userUserId: "ledger-api-user",
			};
		}
	}
}

async function withLocalNetAuth(
	scenario: LocalNetTwoParticipantsScenario,
	fixtureDir: string,
): Promise<LocalNetTwoParticipantsScenario> {
	switch (scenario.auth.mode) {
		case "auto": {
			const auth = await generatedLocalNetAuth();
			await writeJson(join(fixtureDir, "auth-mode.json"), {
				authMode: auth.authMode,
				source: "generated",
			});

			return {
				...scenario,
				auth: {
					mode: "bearer",
					providerToken: auth.providerToken,
					userToken: auth.userToken,
					providerUserId: auth.providerUserId,
					userUserId: auth.userUserId,
				},
			};
		}
		case "none":
			await writeJson(join(fixtureDir, "auth-mode.json"), {
				authMode: "none",
				source: "scenario",
			});
			return scenario;
		case "bearer": {
			const providerToken =
				scenario.auth.providerToken ??
				(scenario.auth.providerTokenFile
					? await Bun.file(scenario.auth.providerTokenFile).text()
					: undefined) ??
				scenario.auth.token ??
				(scenario.auth.tokenFile
					? await Bun.file(scenario.auth.tokenFile).text()
					: undefined);
			const userToken =
				scenario.auth.userToken ??
				(scenario.auth.userTokenFile
					? await Bun.file(scenario.auth.userTokenFile).text()
					: undefined) ??
				scenario.auth.token ??
				(scenario.auth.tokenFile
					? await Bun.file(scenario.auth.tokenFile).text()
					: undefined);

			await writeJson(join(fixtureDir, "auth-mode.json"), {
				authMode: "provided-bearer",
				source: "scenario",
			});
			return {
				...scenario,
				auth: {
					...scenario.auth,
					providerUserId:
						scenario.auth.providerUserId ??
						scenario.auth.userId ??
						(providerToken ? jwtSubject(providerToken.trim()) : undefined),
					userUserId:
						scenario.auth.userUserId ??
						scenario.auth.userId ??
						(userToken ? jwtSubject(userToken.trim()) : undefined),
				},
			};
		}
	}
}

function commandUserId(
	scenario: LocalNetTwoParticipantsScenario,
	role: "provider" | "user",
): string {
	switch (scenario.auth.mode) {
		case "auto":
		case "none":
			return role === "provider" ? "trace-probe-provider" : "trace-probe-user";
		case "bearer": {
			const userId =
				role === "provider"
					? (scenario.auth.providerUserId ?? scenario.auth.userId)
					: (scenario.auth.userUserId ?? scenario.auth.userId);
			if (!userId) {
				throw new Error(
					`No ${role} Ledger API user id available for command submission`,
				);
			}

			return userId;
		}
	}
}

async function captureSandbox(): Promise<void> {
	const scenario = await loadScenario("sandbox");
	if (scenario.scenario !== "sandbox") {
		throw new Error(`Expected sandbox scenario, got ${scenario.scenario}`);
	}

	const fixtureDir = join(projectRoot, "fixtures", "sandbox");

	await Bun.$`mkdir -p ${fixtureDir}`;

	const darPath = await buildProbe();
	const packageId = await mainPackageId(darPath);

	const version = await ledgerApiVersion(scenario);
	await writeJson(join(fixtureDir, "ledger-api-version.json"), version);

	const before = await ledgerEnd(scenario);
	await writeJson(join(fixtureDir, "ledger-end-before.json"), before);

	const runId = Date.now();
	const operator = await allocateParty(
		scenario,
		"single",
		`${scenario.parties.operatorHint}${runId}`,
	);
	const counterparty = await allocateParty(
		scenario,
		"single",
		`${scenario.parties.counterpartyHint}${runId}`,
	);
	await writeJson(join(fixtureDir, "parties.json"), { operator, counterparty });

	const createSubmit = await submitCreate(
		scenario,
		"single",
		packageId,
		operator,
		counterparty,
	);
	await writeJson(join(fixtureDir, "create-submit.json"), createSubmit);

	const createUpdateId = updateId(createSubmit);
	const probeContractId = createdContractId(createSubmit);
	const createUpdate = await getUpdateById(scenario, "single", createUpdateId, [
		operator,
		counterparty,
	]);
	await writeJson(join(fixtureDir, "create-update.json"), createUpdate);

	const exerciseSubmit = await submitAccept(
		scenario,
		"single",
		packageId,
		counterparty,
		probeContractId,
	);
	await writeJson(join(fixtureDir, "exercise-submit.json"), exerciseSubmit);

	const exerciseUpdateId = updateId(exerciseSubmit);
	const exerciseUpdate = await getUpdateById(
		scenario,
		"single",
		exerciseUpdateId,
		[operator, counterparty],
	);
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
			transaction(createUpdate).synchronizerId ??
			transaction(createUpdate).synchronizer_id,
		exerciseSynchronizerId:
			transaction(exerciseUpdate).synchronizerId ??
			transaction(exerciseUpdate).synchronizer_id,
		createEventKinds: eventKinds(createUpdate),
		exerciseEventKinds: eventKinds(exerciseUpdate),
	};

	await writeJson(join(fixtureDir, "summary.json"), summary);
	console.log(JSON.stringify(summary, null, 2));
}

async function captureLocalNetTwoParticipants(): Promise<void> {
	const loadedScenario = await loadScenario("localnet-two-participants");
	if (loadedScenario.scenario !== "localnet-two-participants") {
		throw new Error(
			`Expected localnet-two-participants scenario, got ${loadedScenario.scenario}`,
		);
	}

	const fixtureDir = join(projectRoot, "fixtures", "localnet-two-participants");
	await Bun.$`mkdir -p ${fixtureDir}`;
	const scenario = await withLocalNetAuth(loadedScenario, fixtureDir);

	const darPath = await buildProbe();
	const packageId = await mainPackageId(darPath);

	const providerVersion = await ledgerApiVersionForRole(scenario, "provider");
	await writeJson(
		join(fixtureDir, "ledger-api-version-provider.json"),
		providerVersion,
	);

	const userVersion = await ledgerApiVersionForRole(scenario, "user");
	await writeJson(
		join(fixtureDir, "ledger-api-version-user.json"),
		userVersion,
	);

	const providerBefore = await ledgerEndForRole(scenario, "provider");
	await writeJson(
		join(fixtureDir, "ledger-end-before-provider.json"),
		providerBefore,
	);

	const userBefore = await ledgerEndForRole(scenario, "user");
	await writeJson(join(fixtureDir, "ledger-end-before-user.json"), userBefore);

	if (scenario.uploadDar) {
		await writeJson(
			join(fixtureDir, "upload-dar-provider.json"),
			await uploadDar(scenario, "provider", darPath),
		);
		await writeJson(
			join(fixtureDir, "upload-dar-user.json"),
			await uploadDar(scenario, "user", darPath),
		);
	}

	const runId = Date.now();
	const operator = await allocateParty(
		scenario,
		"provider",
		`${scenario.parties.operatorHint}${runId}`,
	);
	const counterparty = await allocateParty(
		scenario,
		"user",
		`${scenario.parties.counterpartyHint}${runId}`,
	);
	const uninvolved = await allocateParty(
		scenario,
		"provider",
		`TraceUninvolved${runId}`,
	);
	await writeJson(join(fixtureDir, "parties.json"), {
		operator,
		counterparty,
		uninvolved,
	});

	const providerUserId = commandUserId(scenario, "provider");
	const userUserId = commandUserId(scenario, "user");
	await writeJson(
		join(fixtureDir, "user-rights-provider.json"),
		await ensureUserWithPartyRights(
			scenario,
			"provider",
			providerUserId,
			operator,
		),
	);
	await writeJson(
		join(fixtureDir, "user-rights-user.json"),
		await ensureUserWithPartyRights(scenario, "user", userUserId, counterparty),
	);
	await writeJson(
		join(fixtureDir, "user-rights-uninvolved.json"),
		await ensureUserWithPartyRights(
			scenario,
			"provider",
			providerUserId,
			uninvolved,
		),
	);

	await writeResult(fixtureDir, "connected-synchronizers-provider.json", () =>
		connectedSynchronizers(scenario, "provider", operator),
	);
	await writeResult(fixtureDir, "connected-synchronizers-user.json", () =>
		connectedSynchronizers(scenario, "user", counterparty),
	);
	await writeResult(fixtureDir, "connected-synchronizers-uninvolved.json", () =>
		connectedSynchronizers(scenario, "provider", uninvolved),
	);

	// This create transaction is sent by the operator party, through the "provider"
	// participant node. When the ledger request is sent internally, it will attach
	// the bearer token for the user that can act as the operator party.
	const createSubmit = await submitCreate(
		scenario,
		"provider",
		packageId,
		operator,
		counterparty,
		[operator],
		providerUserId,
	);
	await writeJson(
		join(fixtureDir, "create-submit-provider.json"),
		createSubmit,
	);

	const createUpdateId = updateId(createSubmit);
	const probeContractId = createdContractId(createSubmit);

	const createProvider = await retry("fetch create from provider", () =>
		getUpdateById(scenario, "provider", createUpdateId, [operator]),
	);
	await writeJson(
		join(fixtureDir, "create-update-provider.json"),
		createProvider,
	);

	const createUser = await retry("fetch create from user", () =>
		getUpdateById(scenario, "user", createUpdateId, [counterparty]),
	);
	await writeJson(join(fixtureDir, "create-update-user.json"), createUser);

	// The uninvolved party has read rights, but is not a witness. This captures
	// Canton's privacy behaviour separately from authentication/authorisation.
	const createUninvolved = await writeResult(
		fixtureDir,
		"create-update-uninvolved.json",
		() => getUpdateById(scenario, "provider", createUpdateId, [uninvolved]),
	);

	const exerciseSubmit = await retry("submit exercise from user", () =>
		submitAccept(
			scenario,
			"user",
			packageId,
			counterparty,
			probeContractId,
			userUserId,
		),
	);
	await writeJson(
		join(fixtureDir, "exercise-submit-user.json"),
		exerciseSubmit,
	);

	const exerciseUpdateId = updateId(exerciseSubmit);

	const exerciseProvider = await retry("fetch exercise from provider", () =>
		getUpdateById(scenario, "provider", exerciseUpdateId, [operator]),
	);
	await writeJson(
		join(fixtureDir, "exercise-update-provider.json"),
		exerciseProvider,
	);

	const exerciseUser = await retry("fetch exercise from user", () =>
		getUpdateById(scenario, "user", exerciseUpdateId, [counterparty]),
	);
	await writeJson(join(fixtureDir, "exercise-update-user.json"), exerciseUser);

	const exerciseUninvolved = await writeResult(
		fixtureDir,
		"exercise-update-uninvolved.json",
		() => getUpdateById(scenario, "provider", exerciseUpdateId, [uninvolved]),
	);

	const summary = {
		scenario: scenario.scenario,
		providerLedger: ledgerForRole(scenario, "provider"),
		userLedger: ledgerForRole(scenario, "user"),
		tls: !scenario.plaintext,
		auth: scenario.auth.mode,
		providerUserId,
		userUserId,
		operator,
		counterparty,
		uninvolved,
		createUpdateId,
		exerciseUpdateId,
		createSynchronizerId:
			transaction(createProvider).synchronizerId ??
			transaction(createProvider).synchronizer_id,
		exerciseSynchronizerId:
			transaction(exerciseUser).synchronizerId ??
			transaction(exerciseUser).synchronizer_id,
		createProviderEventKinds: eventKinds(createProvider),
		createUserEventKinds: eventKinds(createUser),
		createUninvolvedEventKinds: visibleEventKinds(createUninvolved),
		createUninvolvedVisible: visibleEventKinds(createUninvolved).length > 0,
		exerciseProviderEventKinds: eventKinds(exerciseProvider),
		exerciseUserEventKinds: eventKinds(exerciseUser),
		exerciseUninvolvedEventKinds: visibleEventKinds(exerciseUninvolved),
		exerciseUninvolvedVisible: visibleEventKinds(exerciseUninvolved).length > 0,
	};

	await writeJson(join(fixtureDir, "summary.json"), summary);
	console.log(JSON.stringify(summary, null, 2));
}

async function main(): Promise<void> {
	const scenarioName = process.argv[2];
	switch (scenarioName) {
		case "sandbox":
			await captureSandbox();
			return;
		case "localnet-two-participants":
			await captureLocalNetTwoParticipants();
			return;
		default:
			console.error(
				"Usage: bun scripts/capture.ts sandbox|localnet-two-participants",
			);
			process.exit(1);
	}
}

await main();
