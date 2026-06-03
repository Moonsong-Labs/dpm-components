import { existsSync } from "node:fs";
import { resolve } from "node:path";

type HealthTarget = {
  name: string;
  url: string;
};

const defaultQuickstartDir = "../../../cn-quickstart/quickstart";

const healthTargets: HealthTarget[] = [
  { name: "app-user", url: "http://localhost:2903/api/validator/readyz" },
  { name: "app-provider", url: "http://localhost:3903/api/validator/readyz" },
  { name: "sv", url: "http://localhost:4903/api/validator/readyz" },
];

type Command = "start" | "stop" | "clean" | "status";

type Options = {
  command: Command;
  quickstartDir: string;
  skipBuild: boolean;
};

function usage(): string {
  return [
    "Usage: bun scripts/localnet.ts start|stop|clean|status [--quickstart <path>] [--skip-build]",
    "",
    `Default quickstart path: ${defaultQuickstartDir}`,
    "Override with --quickstart or CN_QUICKSTART_DIR.",
  ].join("\n");
}

function parseOptions(argv: string[]): Options {
  const [commandArg, ...rest] = argv;
  if (
    commandArg !== "start" &&
    commandArg !== "stop" &&
    commandArg !== "clean" &&
    commandArg !== "status"
  ) {
    console.error(usage());
    process.exit(1);
  }

  let quickstartDir = process.env.CN_QUICKSTART_DIR ?? defaultQuickstartDir;
  let skipBuild = false;
  for (let index = 0; index < rest.length; index += 1) {
    const arg = rest[index];
    if (arg === "--quickstart") {
      const value = rest[index + 1];
      if (!value) {
        console.error("--quickstart requires a path");
        process.exit(1);
      }
      quickstartDir = value;
      index += 1;
      continue;
    }

    if (arg.startsWith("--quickstart=")) {
      quickstartDir = arg.slice("--quickstart=".length);
      continue;
    }

    if (arg === "--skip-build") {
      skipBuild = true;
      continue;
    }

    console.error(`Unknown argument: ${arg}`);
    console.error(usage());
    process.exit(1);
  }

  return {
    command: commandArg,
    quickstartDir: resolve(process.cwd(), quickstartDir),
    skipBuild,
  };
}

function assertQuickstartDir(quickstartDir: string): void {
  if (!existsSync(quickstartDir)) {
    throw new Error(
      [
        "Could not find cn-quickstart's quickstart directory.",
        "",
        `Expected it at: ${quickstartDir}`,
        "",
        `Clone cn-quickstart separately, or pass --quickstart <path>.`,
      ].join("\n"),
    );
  }
}

function assertSetupComplete(quickstartDir: string): void {
  const envLocal = resolve(quickstartDir, ".env.local");
  if (existsSync(envLocal)) {
    return;
  }

  throw new Error(
    [
      "cn-quickstart is present but does not look set up yet.",
      "",
      `Expected .env.local at: ${envLocal}`,
      "",
      "Run this once from the quickstart directory:",
      "  make setup",
      "",
      "Then retry:",
      "  bun localnet start",
    ].join("\n"),
  );
}

async function runMake(
  quickstartDir: string,
  target: "build" | "start" | "stop" | "clean-docker",
): Promise<void> {
  const envrc = resolve(quickstartDir, ".envrc");
  const command = existsSync(envrc)
    ? ["direnv", "exec", quickstartDir, "make", target]
    : ["make", target];

  if (existsSync(envrc) && !(await commandExists("direnv"))) {
    throw new Error(
      [
        "cn-quickstart has a .envrc, but direnv is not available to this script.",
        "",
        "Install direnv, then allow the Quickstart environment once:",
        `  cd ${quickstartDir}`,
        "  direnv allow",
      ].join("\n"),
    );
  }

  const proc = Bun.spawn(command, {
    cwd: quickstartDir,
    stdin: "inherit",
    stdout: "inherit",
    stderr: "inherit",
  });
  const exitCode = await proc.exited;
  if (exitCode !== 0) {
    process.exit(exitCode);
  }
}

async function commandExists(command: string): Promise<boolean> {
  try {
    const proc = Bun.spawn([command, "--version"], {
      stdout: "ignore",
      stderr: "ignore",
    });
    return (await proc.exited) === 0;
  } catch {
    return false;
  }
}

async function checkHealth({ name, url }: HealthTarget): Promise<boolean> {
  try {
    const response = await fetch(url);
    const healthy = response.ok;
    console.log(`${healthy ? "ok" : "failed"} ${name} ${url} ${response.status}`);
    return healthy;
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    console.log(`failed ${name} ${url} ${message}`);
    return false;
  }
}

async function start(quickstartDir: string, skipBuild: boolean): Promise<void> {
  assertQuickstartDir(quickstartDir);
  assertSetupComplete(quickstartDir);
  if (!skipBuild) {
    await runMake(quickstartDir, "build");
  }
  await runMake(quickstartDir, "start");
}

async function stop(quickstartDir: string): Promise<void> {
  assertQuickstartDir(quickstartDir);
  await runMake(quickstartDir, "stop");
}

async function clean(quickstartDir: string): Promise<void> {
  assertQuickstartDir(quickstartDir);
  console.log("Running make clean-docker. This removes LocalNet containers and volumes.");
  await runMake(quickstartDir, "clean-docker");
}

async function status(): Promise<void> {
  const results = await Promise.all(healthTargets.map(checkHealth));
  if (results.some((healthy) => !healthy)) {
    process.exit(1);
  }
}

async function main(): Promise<void> {
  const { command, quickstartDir, skipBuild } = parseOptions(process.argv.slice(2));

  switch (command) {
    case "start":
      await start(quickstartDir, skipBuild);
      return;
    case "stop":
      await stop(quickstartDir);
      return;
    case "clean":
      await clean(quickstartDir);
      return;
    case "status":
      await status();
      return;
  }
}

try {
  await main();
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
}
