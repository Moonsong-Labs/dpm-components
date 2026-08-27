# Trace Probe

Small Ledger API probe for testing how Daml transactions appear through `dpm sandbox` and a `cn-quickstart` LocalNet.

The scripts build a tiny Daml package, create parties, submit a `Probe` contract, exercise `Accept`, fetch the resulting Ledger API updates, and write the raw JSON evidence under `fixtures/`.

## Quick Start

Install dependencies:

```bash
cd examples/trace-probe
bun install
```

Run the sandbox scenario:

```bash
bun sandbox
bun trace:sandbox
```

Run the LocalNet scenario:

```bash
bun localnet:start
bun localnet:status
bun trace:localnet
bun localnet:stop
```

For a fast restart when `cn-quickstart` is already built:

```bash
bun localnet:start --skip-build
```

Reset LocalNet state:

```bash
bun localnet:clean
```

## LocalNet Assumptions

`bun localnet:*` assumes `cn-quickstart` lives at:

```text
../../../cn-quickstart/quickstart
```

Override that with:

```bash
bun localnet:start --quickstart /path/to/cn-quickstart/quickstart
```

Before using the wrapper, run `make setup` once from `cn-quickstart/quickstart`. If that directory has a `.envrc`, the wrapper uses `direnv exec` so local `JAVA_HOME` and `PATH` settings are honoured.

## Results

Scenario output is written to:

```text
fixtures/sandbox/
fixtures/localnet-two-participants/
```

Start with `summary.json`. It records the parties, update ids, synchronizer ids, and event kinds seen by each role.

Useful files:

- `parties.json`: allocated Daml parties.
- `auth-mode.json`: detected LocalNet auth mode.
- `create-submit-*.json`: command submission response for creating `Probe`.
- `create-update-*.json`: fetched create transaction/update.
- `exercise-submit-*.json`: command submission response for exercising `Accept`.
- `exercise-update-*.json`: fetched exercise transaction/update.
- `*-uninvolved-error.json`: expected privacy check failures for the uninvolved party.

## How To Read The Results

The Daml model is intentionally small:

- `Probe` is signed by `operator`.
- `counterparty` observes `Probe`.
- `counterparty` exercises `Accept`.
- `ProbeResult` is signed by both parties.

In the LocalNet scenario:

- `operator` is allocated on the App Provider participant.
- `counterparty` is allocated on the App User participant.
- `uninvolved` is allocated and granted read rights, but is not a stakeholder or witness.

Provider and user update files should contain transaction events because those parties are involved. The uninvolved party checks should not reveal the transaction. That demonstrates the Canton/Daml privacy rule: Ledger API authorisation lets a user query as a party, but visibility still depends on whether that party is entitled to see the transaction.

## Scenarios

`sandbox` uses `dpm sandbox` on `localhost:6865`, with no Ledger API auth.

`localnet-two-participants` uses `cn-quickstart` LocalNet:

- App Provider Ledger API: `localhost:3901`
- App User Ledger API: `localhost:2901`
- Auth mode: auto-detected (`oauth2` or `shared-secret`)
- DAR upload: enabled

## Troubleshooting

- `Expected .env.local`: run `make setup` once in `cn-quickstart/quickstart`.
- `target dpm-sdk version not installed`: LocalNet goes through `cn-quickstart`, so this example must use the same Daml SDK that Quickstart pins. Read `sdk-version` in `cn-quickstart/quickstart/daml/licensing/daml.yaml` (also `DAML_RUNTIME_VERSION` in `.env`), then `dpm install` that version. Do not bump Quickstart to whatever SDK is active in this repository; keep this repository in step with Quickstart instead.
- Java `26.0.1` Gradle failure: use JDK 17 or 21 for Quickstart, usually via `.envrc` and `direnv allow`.
- `direnv is not available`: install `direnv`, then run `direnv allow` in `cn-quickstart/quickstart`.
- `Unauthenticated`: check LocalNet is running and `auth-mode.json` can be generated.
- `PermissionDenied`: inspect `user-rights-*.json`; the script grants `CanActAs` and `CanReadAs` to generated parties.
- LocalNet not ready: run `bun localnet:status`.
- Stale state: run `bun localnet:clean`, then start again.
- Missing `grpcurl`: install it before running trace scripts.

## What Is Built

- `main/daml/TraceProbe.daml`: small Daml package used for create/exercise transactions.
- `scripts/capture.ts`: scenario orchestration and fixture writing.
- `scripts/ledger-api.ts`: low-level `grpcurl` Ledger API helpers.
- `scripts/localnet.ts`: `cn-quickstart` wrapper for start, stop, clean, and status.
- `scenarios/*.json`: scenario configuration.
- `fixtures/`: generated JSON evidence from each run.
