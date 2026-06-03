type HealthTarget = {
	name: string;
	url: string;
};

const healthTargets: HealthTarget[] = [
	{ name: "app-user", url: "http://localhost:2903/api/validator/readyz" },
	{ name: "app-provider", url: "http://localhost:3903/api/validator/readyz" },
	{ name: "sv", url: "http://localhost:4903/api/validator/readyz" },
];

async function checkHealth({ name, url }: HealthTarget): Promise<boolean> {
	try {
		const response = await fetch(url);
		const healthy = response.ok;
		console.log(
			`${healthy ? "ok" : "failed"} ${name} ${url} ${response.status}`,
		);
		return healthy;
	} catch (error) {
		const message = error instanceof Error ? error.message : String(error);
		console.log(`failed ${name} ${url} ${message}`);
		return false;
	}
}

async function status(): Promise<void> {
	const results = await Promise.all(healthTargets.map(checkHealth));
	if (results.some((healthy) => !healthy)) {
		process.exit(1);
	}
}

async function main(): Promise<void> {
	const command = process.argv[2];
	if (command !== "status") {
		console.error("Usage: bun scripts/localnet.ts status");
		process.exit(1);
	}

	await status();
}

await main();

export {};
