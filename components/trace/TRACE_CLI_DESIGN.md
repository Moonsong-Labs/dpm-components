# `dpm trace` CLI Design

This document records technical design decisions for the first Rust implementation
of the `trace` DPM component. It is intended as the implementation guide for the
initial `dpm trace <update-id>` command.

## Goals

1. Implement a `trace` component that can be published and installed as a DPM
   component.
2. Start with `dpm trace <update-id>` against the participant Ledger API.
3. Support authorised Ledger API calls against local and remote participant nodes.
4. Provide good interactive UX for developers using the CLI manually.
5. Preserve scriptability for CI, automation, and advanced users.
6. Keep durable configuration separate from secrets.
7. Respect Canton/Daml authorisation and privacy boundaries.

## Authentication

### Profile Files

The CLI uses named profiles to describe how to connect to a participant node and how
to obtain OAuth2 tokens.

Profile lookup order:

1. `--profile-file <path>`, when explicitly provided.
2. Project-local profile file: `.dpm/trace/profiles.toml`.
3. User-global profile file: `$DPM_HOME/trace/profiles.toml`.
4. Built-in local defaults, where appropriate.

Do not store user-authored profiles under a cache path such as
`$DPM_HOME/cache/components/trace/profiles.toml`. Profiles are durable
configuration, not disposable cache data.

Project-local profiles are intended for team-shared, non-secret connection metadata.
User-global profiles are intended for personal defaults. Both profile locations must
avoid storing access tokens, refresh tokens, client secrets, or other credentials.

Example profile:

```toml
[profiles.prod]
ledger = "participant.prod.example.com:443"
tls = true
issuer = "https://login.example.com/realms/daml"
client_id = "dpm-trace"
audience = "https://canton.network.global"
scopes = ["openid"]
party = ["Alice::1220..."]
```

### Profile Creation UX

The CLI should support both fully scripted and interactive profile creation.

Scripted example:

```bash
dpm trace profile add prod \
  --ledger participant.prod.example.com:443 \
  --tls \
  --issuer https://login.example.com/realms/daml \
  --client-id dpm-trace \
  --audience https://canton.network.global
```

Interactive example:

```text
Profile name: prod
Ledger API address [localhost:6865]: participant.prod.example.com:443
Use TLS? [Y/n]: Y
OAuth2 issuer URL []: https://login.example.com/realms/daml
OAuth2 client id [dpm-trace]: dpm-trace
OAuth2 audience [https://canton.network.global]:
Default parties, comma separated []: Alice::1220...
Profile saved. Log in now? [Y/n]:
```

Fields passed as CLI arguments should pre-fill or skip the corresponding prompts.
Prompts should use empty defaults or sensible local-development defaults.

Creating a profile writes only non-secret configuration to the selected profile file.
It does not authenticate the user unless they choose to log in immediately.

### Token Storage

OAuth2 tokens must be stored in the operating system keychain, not in the profile
file.

Recommended key shape:

```text
service: dpm-trace
account: <profile-name>:<issuer>:<client-id>
```

Stored values should include:

1. Access token.
2. Refresh token, if issued.
3. Access token expiry time.

The first implementation should prefer a Rust keychain integration, such as the
`keyring` crate. If a filesystem fallback is ever added, it must be explicit,
clearly documented, and treated as less secure.

### OAuth2 Flow

The first version should implement browser-based OAuth2 Authorization Code with
PKCE.

`dpm trace login --profile <name>` should:

1. Load the selected profile.
2. Discover OAuth2 endpoints from the issuer metadata.
3. Generate a PKCE verifier and challenge.
4. Start a temporary HTTP callback server on `127.0.0.1:<random-port>`.
5. Open the user's browser to the issuer's authorisation URL.
6. Wait for the browser redirect to the local callback URL.
7. Validate the returned `state`.
8. Exchange the authorisation code and PKCE verifier for tokens.
9. Store tokens in the OS keychain.
10. Shut down the temporary callback server.

The temporary localhost callback server exists only so the browser can deliver the
authorisation code back to the CLI. It is started during login and stopped as soon
as the token exchange completes or fails.

The callback URL is local to the user's browser and CLI, not to the remote
participant node or the remote OAuth2 server. This works for remote issuers because
the issuer redirects the user's browser to `127.0.0.1`; the issuer does not need to
open a network connection to the user's machine.

The OAuth2 client registered with the issuer must allow loopback redirect URIs, for
example:

```text
http://127.0.0.1:<port>/callback
```

Some identity providers require exact redirect URIs. If dynamic ports are not
allowed, the profile may need a fixed callback port field in a later iteration.

### Remote Participant Nodes

This mechanism works for remote participant nodes when the participant is configured
to accept bearer tokens issued by the configured OAuth2 issuer.

The CLI obtains an access token from the issuer and sends it to the Ledger API as
gRPC metadata:

```text
authorization: Bearer <access-token>
```

The participant validates the token according to its own auth configuration,
including issuer, JWKS, audience, expiry, subject, and ledger user mapping.

For `dpm trace <update-id>`, a valid token is not enough. The authenticated ledger
user must also have `CanReadAs` rights for the parties used in the update format.

The CLI should distinguish these error classes:

1. `UNAUTHENTICATED`: token missing, expired, malformed, wrong issuer, wrong
   audience, or otherwise rejected.
2. `PERMISSION_DENIED`: token is valid, but the ledger user lacks the required
   Ledger API rights.
3. Privacy-filtered results: the request is authorised, but the participant can only
   return events visible to the requested parties.

The trace command should describe its result as the visible transaction view for the
requested parties, not as a global transaction view.

### Authenticated Command Flow

Expected first-version user flow:

1. Create or select a profile.

   ```bash
   dpm trace profile add prod
   ```

   Consequence: the CLI writes non-secret connection and OAuth2 metadata to the
   chosen profile file.

2. Log in.

   ```bash
   dpm trace login --profile prod
   ```

   Consequence: the CLI runs Authorization Code with PKCE, receives OAuth2 tokens,
   stores them in the OS keychain, and leaves the profile file unchanged except for
   non-secret metadata.

3. Run an authenticated trace command.

   ```bash
   dpm trace <update-id> --profile prod --party Alice::1220...
   ```

   Consequence: the CLI loads the profile, retrieves tokens from the keychain,
   refreshes the access token if needed and possible, opens a Ledger API gRPC
   connection, attaches the bearer token, and calls `UpdateService/GetUpdateById`
   with an update format scoped to the requested parties.

4. Render the response.

   Consequence: if the call succeeds, the CLI renders the visible transaction view.
   If authentication or authorisation fails, the CLI explains whether the user needs
   to log in again or needs additional ledger rights.

### Initial Non-Interactive Support

Even though the main UX is profile plus browser login, the first implementation
should keep a scriptable token path for advanced users:

1. `--access-token-file <path>`.
2. `--access-token-command <command>`.
3. `DPM_TRACE_ACCESS_TOKEN`.

These inputs should be treated as overrides for the current invocation and should
not be written into profile files.

### Out Of Scope For The First Auth Pass

1. Device authorisation fallback for headless environments.
2. Shared-secret LocalNet token generation as a polished user-facing flow.
3. Automatic party discovery from token claims.
4. Admin flows for granting ledger user rights.
5. Multi-profile token migration or profile sync.

## Trace Output

The first version should focus on a polished human-readable terminal output. Machine
readable formats such as `json` and `raw-json` are useful, but should be kept for a
later implementation pass.

### Output Principles

1. Optimise the default output for scanning and debugging.
2. Show transaction metadata before event details.
3. Render the participant's visible transaction view, not an implied global view.
4. Use Daml-oriented labels such as `create`, `exercise`, and `archive`.
5. Hide package IDs and low-level Ledger API fields by default when a shorter Daml
   name is available.
6. Preserve enough node IDs, contract IDs, parties, and choice names to make the
   trace actionable.
7. Use colour only when stdout is a TTY. Plain output must remain clean in logs.

### First-Version Format

The default format is a pretty terminal renderer. The initial CLI does not need a
`--format` flag unless we want to reserve the syntax. If the flag is added early, the
only implemented value should be `pretty`.

Example:

```text
Transaction 1220...9ab3
Record time:    2026-06-04T15:12:09Z
Effective at:   2026-06-04T15:12:08Z
Synchronizer:   global-domain::...
Offset:         00000000000042
Visible as:     Alice, Bank

Events
[0] exercise Bank:Account.AcceptTransfer
    actor: Alice
    contract: #1:0
    consuming: yes

    argument
      amount: 100.00
      currency: "GBP"

    children
    [1] archive Bank:Account
        contract: #1:0

    [2] create Bank:Account
        contract: #2:0
        signatories: Bank
        observers: Alice

        payload
          owner: Alice
          balance: 100.00
```

### Header

The header should include the most useful transaction metadata:

1. Update ID.
2. Record time, if present.
3. Effective time, if present.
4. Synchronizer ID, if present.
5. Offset, if present.
6. Command ID and workflow ID, when present and non-empty.
7. Requested parties, labelled as `Visible as`.

The header should make it clear which parties scoped the request. This is important
because `GetUpdateById` returns the view visible to the requested parties and the
authorised ledger user.

### Events

The first renderer should support the transaction event types needed by
`TRANSACTION_SHAPE_LEDGER_EFFECTS`:

1. `created` events as `create <Template>`.
2. `exercised` events as `exercise <Template>.<Choice>`.
3. `archived` events as `archive <Template>`.

Each event should include the fields that are most useful for workflow debugging:

1. Node ID.
2. Template name.
3. Contract ID.
4. Acting parties for exercises.
5. Consuming flag for exercises.
6. Signatories and observers for creates.
7. Choice argument for exercises.
8. Exercise result when present and reasonably small.
9. Create payload for creates.

Witness parties, package IDs, trace context, interface IDs, and full template IDs
should be reserved for verbose output.

### Event Tree

The renderer should represent exercise children as an indented tree. The internal
tree should be built from event node IDs and `lastDescendantNodeId`, not from array
order alone.

Array order can still be used for display once the parent-child structure has been
derived. If the response is incomplete because of privacy filtering, the renderer
should tolerate missing parents or missing children and display the visible events
without implying that hidden events do not exist.

### Daml Value Rendering

Daml values should be rendered in a compact, readable syntax rather than raw
protobuf JSON.

Guidelines:

1. Records should render as labelled fields.
2. Variants should show constructor name and value.
3. Lists should use compact single-line output when short and multi-line output when
   long.
4. Optional values should render as `None` or `Some <value>`.
5. Parties, text, timestamps, dates, decimals, and contract IDs should use their
   natural Daml-like representation.
6. Deeply nested values should remain readable through indentation.

Large values may need truncation in the future, but the first version can render the
full visible value.

### Compact And Verbose Modes

The first implementation should support the default output and may include
`--verbose` if it is cheap to wire through.

Default output:

1. Short template names.
2. Human-oriented metadata.
3. Payloads and choice arguments.
4. Signatories, observers, actors, and consuming flags.

Verbose output:

1. Full package IDs.
2. Full template IDs.
3. Witness parties.
4. Trace context.
5. Interface IDs, if present.
6. Raw node IDs and descendant boundaries when helpful.

A future `--compact` flag can suppress large payloads and focus on event shape, but
it is not required for the first implementation.

### Deferred Formats

The following formats are intentionally deferred:

1. `json`: a stable, tool-friendly JSON representation based on the CLI's internal
   trace model.
2. `raw-json`: the raw Ledger API response encoded as JSON.
3. `markdown`: a possible documentation-friendly rendering.

When these are added, `json` should not simply mirror the protobuf response. It
should expose the stable internal trace model so agent skills and tests can depend
on it.

## Rust Project Structure

The first implementation should keep the Rust project simple. Use clear conceptual
boundaries, but avoid a deep tree of small files until there is enough code to
justify it.

### CLI Library

Use `clap` for command-line parsing.

Recommended dependency shape:

```toml
clap = { version = "...", features = ["derive"] }
```

`clap` is the standard Rust choice for this kind of CLI. It provides reliable
subcommand parsing, generated help text, typed arguments, environment variable
integration, shell completion support, and good derive ergonomics.

This is a better fit than hand-written parsing because the command surface is
expected to grow:

```bash
dpm trace <update-id>
dpm trace login
dpm trace logout
dpm trace profile add
dpm trace watch
```

### Initial File Layout

Start with one file per major concern:

```text
components/trace/
  Cargo.toml
  component.yaml
  src/
    main.rs
    cli.rs
    config.rs
    auth.rs
    ledger.rs
    trace.rs
```

Responsibilities:

1. `main.rs`: entrypoint, top-level error handling, and command dispatch.
2. `cli.rs`: `clap` command and flag definitions.
3. `config.rs`: profile loading, profile precedence, and flag/profile merging.
4. `auth.rs`: token resolution, OAuth2 login flow, and keychain storage.
5. `ledger.rs`: Ledger API client setup, gRPC metadata, TLS/plaintext connection
   handling, and `UpdateService/GetUpdateById`.
6. `trace.rs`: conversion from Ledger API response into a displayable trace and the
   pretty terminal renderer.

### Ledger Boundary

Keep `ledger.rs` as a separate module because Ledger API concerns are different
from trace rendering concerns.

`ledger.rs` should own:

1. `tonic` client construction.
2. Generated Ledger API protobuf types.
3. Bearer token metadata attachment.
4. TLS and plaintext channel configuration.
5. The `GetUpdateById` request shape.
6. Ledger API error mapping where the raw gRPC status is still useful.

`trace.rs` should own:

1. The internal trace model used by the renderer.
2. Event tree construction.
3. Daml value rendering.
4. Pretty terminal output.

This boundary keeps protobuf and gRPC details from leaking through the whole
application, while still avoiding an over-complicated module tree.

### Refactoring Rule

Do not split `auth.rs`, `ledger.rs`, or `trace.rs` into directories at the start.
Refactor only once a file becomes difficult to navigate or once a boundary becomes
obvious from real code.

Likely future splits, if needed:

1. `auth/oauth.rs` and `auth/token_store.rs`.
2. `ledger/client.rs` and `ledger/update_service.rs`.
3. `trace/model.rs`, `trace/tree.rs`, and `trace/render.rs`.

These are not part of the initial structure.
