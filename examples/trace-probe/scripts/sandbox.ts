import { buildProbe, probeDarPath, projectRoot } from "./ledger-api";

async function main(): Promise<void> {
	const command = process.argv[2];
	if (command !== "start") {
		console.error("Usage: bun scripts/sandbox.ts start");
		process.exit(1);
	}

	await buildProbe();

	const proc = Bun.spawn(["dpm", "sandbox", "--dar", probeDarPath], {
		cwd: projectRoot,
		stdin: "inherit",
		stdout: "inherit",
		stderr: "inherit",
	});

	process.exit(await proc.exited);
}

await main();
