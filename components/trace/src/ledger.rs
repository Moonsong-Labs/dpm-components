//! Minimal Ledger API client and trace renderer.
//!
//! This first slice calls `UpdateService/GetUpdateById` and renders transaction
//! metadata plus a compact event summary. It intentionally models only the
//! protobuf fields needed for that path.

use std::collections::HashMap;

use prost::Message;
use prost_types::Timestamp;
use tonic::{
    codegen::http::uri::PathAndQuery, metadata::MetadataValue, transport::Endpoint, Request,
};

use crate::{
    auth,
    cli::Cli,
    config::{self, LoadedProfile},
};

/// Fetch and render one Ledger API update by id.
pub fn trace_update(args: &Cli) -> Result<(), String> {
    let update_id = args
        .update_id
        .clone()
        .ok_or_else(|| "missing update id".to_owned())?;
    let loaded = config::load_profile(args.profile.clone(), args.profile_file.clone())?;
    let parties = trace_parties(args, &loaded)?;
    let access_token = auth::access_token(&loaded.name, &loaded.profile)?;

    if loaded.profile.tls {
        return Err(
            "TLS Ledger API connections are not wired in this first trace slice".to_owned(),
        );
    }

    // We create a new tokio runtime to run the async get_update_by_id call, and
    // block on it to get the response.
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|error| format!("failed to start async runtime: {error}"))?;
    let response = runtime.block_on(get_update_by_id(
        &loaded.profile.ledger,
        &update_id,
        &parties,
        access_token.as_deref(),
    ))?;

    render_update(&response, &parties)
}

/// Resolve parties from command flags or profile defaults.
fn trace_parties(args: &Cli, loaded: &LoadedProfile) -> Result<Vec<String>, String> {
    let parties = if args.parties.is_empty() {
        loaded.profile.party.clone()
    } else {
        args.parties.clone()
    };

    if parties.is_empty() {
        return Err(
            "no parties configured; pass --party or add default parties to the profile".to_owned(),
        );
    }

    Ok(parties)
}

/// Call UpdateService/GetUpdateById on a plaintext Ledger API endpoint.
async fn get_update_by_id(
    ledger: &str,
    update_id: &str,
    parties: &[String],
    access_token: Option<&str>,
) -> Result<GetUpdateResponse, String> {
    let endpoint = Endpoint::from_shared(format!("http://{ledger}"))
        .map_err(|error| format!("invalid ledger endpoint '{ledger}': {error}"))?;
    let channel = endpoint
        .connect()
        .await
        .map_err(|error| format!("failed to connect to Ledger API at {ledger}: {error}"))?;
    let mut grpc = tonic::client::Grpc::new(channel);
    grpc.ready()
        .await
        .map_err(|error| format!("Ledger API client was not ready: {error}"))?;

    let mut request = Request::new(GetUpdateByIdRequest {
        update_id: update_id.to_owned(),
        update_format: Some(update_format(parties)),
    });
    if let Some(access_token) = access_token {
        let value = MetadataValue::try_from(format!("Bearer {access_token}"))
            .map_err(|error| format!("invalid bearer token metadata: {error}"))?;
        request.metadata_mut().insert("authorization", value);
    }

    let path = PathAndQuery::from_static("/com.daml.ledger.api.v2.UpdateService/GetUpdateById");
    let response = grpc
        .unary(request, path, tonic_prost::ProstCodec::default())
        .await
        .map_err(|error| format!("GetUpdateById failed: {error}"))?;

    Ok(response.into_inner())
}

/// Build the update format used by the trace command.
fn update_format(parties: &[String]) -> UpdateFormat {
    let filters_by_party = parties
        .iter()
        .map(|party| {
            (
                party.clone(),
                Filters {
                    cumulative: vec![CumulativeFilter {
                        identifier_filter: Some(
                            cumulative_filter::IdentifierFilter::WildcardFilter(WildcardFilter {
                                include_created_event_blob: true,
                            }),
                        ),
                    }],
                },
            )
        })
        .collect();

    UpdateFormat {
        include_transactions: Some(TransactionFormat {
            event_format: Some(EventFormat {
                filters_by_party,
                verbose: true,
            }),
            transaction_shape: TransactionShape::LedgerEffects as i32,
        }),
    }
}

/// Render a compact transaction summary.
fn render_update(response: &GetUpdateResponse, parties: &[String]) -> Result<(), String> {
    let Some(get_update_response::Update::Transaction(transaction)) = &response.update else {
        return Err("GetUpdateById returned a non-transaction update".to_owned());
    };

    println!("Transaction {}", transaction.update_id);
    print_optional(
        "Record time",
        transaction.record_time.as_ref().map(format_timestamp),
    );
    print_optional(
        "Effective at",
        transaction.effective_at.as_ref().map(format_timestamp),
    );
    print_optional("Synchronizer", non_empty(&transaction.synchronizer_id));
    print_optional("Offset", Some(transaction.offset.to_string()));
    print_optional("Command ID", non_empty(&transaction.command_id));
    print_optional("Workflow ID", non_empty(&transaction.workflow_id));
    println!("Visible as:     {}", parties.join(", "));
    println!();
    println!("Events");

    if transaction.events.is_empty() {
        println!("(no visible events)");
        return Ok(());
    }

    for event in &transaction.events {
        render_event(event);
    }

    Ok(())
}

/// Print one optional header field.
fn print_optional(label: &str, value: Option<String>) {
    if let Some(value) = value {
        println!("{label:15}{value}");
    }
}

/// Return a string only when the value is not empty.
fn non_empty(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_owned())
    }
}

/// Render one visible transaction event.
fn render_event(event: &Event) {
    match &event.event {
        Some(event::Event::Created(created)) => {
            println!(
                "[{}] create {}",
                created.node_id,
                format_identifier(created.template_id.as_ref())
            );
            println!("    contract: {}", created.contract_id);
            print_party_list("signatories", &created.signatories);
            print_party_list("observers", &created.observers);
        }
        Some(event::Event::Archived(archived)) => {
            println!(
                "[{}] archive {}",
                archived.node_id,
                format_identifier(archived.template_id.as_ref())
            );
            println!("    contract: {}", archived.contract_id);
        }
        Some(event::Event::Exercised(exercised)) => {
            println!(
                "[{}] exercise {}.{}",
                exercised.node_id,
                format_identifier(exercised.template_id.as_ref()),
                exercised.choice
            );
            print_party_list("actor", &exercised.acting_parties);
            println!("    contract: {}", exercised.contract_id);
            println!(
                "    consuming: {}",
                if exercised.consuming { "yes" } else { "no" }
            );
        }
        None => println!("[?] unknown event"),
    }
}

/// Print a labelled list of parties when present.
fn print_party_list(label: &str, parties: &[String]) {
    if !parties.is_empty() {
        println!("    {label}: {}", parties.join(", "));
    }
}

/// Render a Daml identifier in a compact module/entity form.
fn format_identifier(identifier: Option<&Identifier>) -> String {
    let Some(identifier) = identifier else {
        return "<unknown>".to_owned();
    };

    if identifier.module_name.is_empty() && identifier.entity_name.is_empty() {
        return identifier.package_id.clone();
    }

    format!("{}:{}", identifier.module_name, identifier.entity_name)
}

/// Render a protobuf timestamp as seconds plus nanoseconds.
fn format_timestamp(timestamp: &Timestamp) -> String {
    if timestamp.nanos == 0 {
        format!("{}s", timestamp.seconds)
    } else {
        format!("{}.{:09}s", timestamp.seconds, timestamp.nanos)
    }
}

/// Ledger API GetUpdateById request.
#[derive(Clone, PartialEq, Message)]
struct GetUpdateByIdRequest {
    /// Ledger API update id.
    #[prost(string, tag = "1")]
    update_id: String,
    /// Requested update format.
    #[prost(message, optional, tag = "2")]
    update_format: Option<UpdateFormat>,
}

/// Ledger API GetUpdateById response.
#[derive(Clone, PartialEq, Message)]
struct GetUpdateResponse {
    /// Returned update.
    #[prost(oneof = "get_update_response::Update", tags = "1, 2, 3")]
    update: Option<get_update_response::Update>,
}

/// Oneof variants for GetUpdateResponse.
mod get_update_response {
    use prost::Oneof;

    use super::Transaction;

    /// Supported update response variants.
    #[derive(Clone, PartialEq, Oneof)]
    pub enum Update {
        /// Transaction update.
        #[prost(message, tag = "1")]
        Transaction(Transaction),
        /// Reassignment update, retained as raw bytes in this first slice.
        #[prost(bytes, tag = "2")]
        Reassignment(Vec<u8>),
        /// Topology transaction update, retained as raw bytes in this first slice.
        #[prost(bytes, tag = "3")]
        TopologyTransaction(Vec<u8>),
    }
}

/// Ledger API update format.
#[derive(Clone, PartialEq, Message)]
struct UpdateFormat {
    /// Transaction format to include.
    #[prost(message, optional, tag = "1")]
    include_transactions: Option<TransactionFormat>,
}

/// Ledger API transaction format.
#[derive(Clone, PartialEq, Message)]
struct TransactionFormat {
    /// Event selection and verbosity.
    #[prost(message, optional, tag = "1")]
    event_format: Option<EventFormat>,
    /// Transaction shape enum value.
    #[prost(enumeration = "TransactionShape", tag = "2")]
    transaction_shape: i32,
}

/// Ledger API transaction shape.
#[derive(Clone, Copy, Debug, Eq, PartialEq, prost::Enumeration)]
enum TransactionShape {
    /// Unspecified transaction shape.
    Unspecified = 0,
    /// ACS delta transaction shape.
    AcsDelta = 1,
    /// Ledger effects transaction shape.
    LedgerEffects = 2,
}

/// Ledger API event format.
#[derive(Clone, PartialEq, Message)]
struct EventFormat {
    /// Party-scoped filters.
    #[prost(map = "string, message", tag = "1")]
    filters_by_party: HashMap<String, Filters>,
    /// Verbose Daml value rendering.
    #[prost(bool, tag = "3")]
    verbose: bool,
}

/// Ledger API filters for one party.
#[derive(Clone, PartialEq, Message)]
struct Filters {
    /// Cumulative filters.
    #[prost(message, repeated, tag = "1")]
    cumulative: Vec<CumulativeFilter>,
}

/// Ledger API cumulative filter.
#[derive(Clone, PartialEq, Message)]
struct CumulativeFilter {
    /// Specific identifier filter.
    #[prost(oneof = "cumulative_filter::IdentifierFilter", tags = "1, 2, 3")]
    identifier_filter: Option<cumulative_filter::IdentifierFilter>,
}

/// Oneof variants for cumulative filters.
mod cumulative_filter {
    use prost::Oneof;

    use super::WildcardFilter;

    /// Supported identifier filters.
    #[derive(Clone, PartialEq, Oneof)]
    pub enum IdentifierFilter {
        /// Match all templates.
        #[prost(message, tag = "1")]
        WildcardFilter(WildcardFilter),
        /// Interface filter placeholder.
        #[prost(bytes, tag = "2")]
        InterfaceFilter(Vec<u8>),
        /// Template filter placeholder.
        #[prost(bytes, tag = "3")]
        TemplateFilter(Vec<u8>),
    }
}

/// Ledger API wildcard filter.
#[derive(Clone, PartialEq, Message)]
struct WildcardFilter {
    /// Include created event blob.
    #[prost(bool, tag = "1")]
    include_created_event_blob: bool,
}

/// Ledger API transaction.
#[derive(Clone, PartialEq, Message)]
struct Transaction {
    /// Assigned update id.
    #[prost(string, tag = "1")]
    update_id: String,
    /// Command id.
    #[prost(string, tag = "2")]
    command_id: String,
    /// Workflow id.
    #[prost(string, tag = "3")]
    workflow_id: String,
    /// Ledger effective time.
    #[prost(message, optional, tag = "4")]
    effective_at: Option<Timestamp>,
    /// Visible events.
    #[prost(message, repeated, tag = "5")]
    events: Vec<Event>,
    /// Absolute offset.
    #[prost(int64, tag = "6")]
    offset: i64,
    /// Synchronizer id.
    #[prost(string, tag = "7")]
    synchronizer_id: String,
    /// Record time.
    #[prost(message, optional, tag = "9")]
    record_time: Option<Timestamp>,
}

/// Ledger API event wrapper.
#[derive(Clone, PartialEq, Message)]
struct Event {
    /// Specific event shape.
    #[prost(oneof = "event::Event", tags = "1, 2, 3")]
    event: Option<event::Event>,
}

/// Oneof variants for events.
mod event {
    use prost::Oneof;

    use super::{ArchivedEvent, CreatedEvent, ExercisedEvent};

    /// Supported event variants.
    #[derive(Clone, PartialEq, Oneof)]
    pub enum Event {
        /// Created event.
        #[prost(message, tag = "1")]
        Created(CreatedEvent),
        /// Archived event.
        #[prost(message, tag = "2")]
        Archived(ArchivedEvent),
        /// Exercised event.
        #[prost(message, tag = "3")]
        Exercised(ExercisedEvent),
    }
}

/// Ledger API created event.
#[derive(Clone, PartialEq, Message)]
struct CreatedEvent {
    /// Node id.
    #[prost(int32, tag = "2")]
    node_id: i32,
    /// Created contract id.
    #[prost(string, tag = "3")]
    contract_id: String,
    /// Created template id.
    #[prost(message, optional, tag = "4")]
    template_id: Option<Identifier>,
    /// Witness parties.
    #[prost(string, repeated, tag = "9")]
    witness_parties: Vec<String>,
    /// Signatories.
    #[prost(string, repeated, tag = "10")]
    signatories: Vec<String>,
    /// Observers.
    #[prost(string, repeated, tag = "11")]
    observers: Vec<String>,
}

/// Ledger API archived event.
#[derive(Clone, PartialEq, Message)]
struct ArchivedEvent {
    /// Node id.
    #[prost(int32, tag = "2")]
    node_id: i32,
    /// Archived contract id.
    #[prost(string, tag = "3")]
    contract_id: String,
    /// Archived template id.
    #[prost(message, optional, tag = "4")]
    template_id: Option<Identifier>,
    /// Witness parties.
    #[prost(string, repeated, tag = "5")]
    witness_parties: Vec<String>,
}

/// Ledger API exercised event.
#[derive(Clone, PartialEq, Message)]
struct ExercisedEvent {
    /// Node id.
    #[prost(int32, tag = "2")]
    node_id: i32,
    /// Target contract id.
    #[prost(string, tag = "3")]
    contract_id: String,
    /// Template id defining the choice.
    #[prost(message, optional, tag = "4")]
    template_id: Option<Identifier>,
    /// Choice name.
    #[prost(string, tag = "5")]
    choice: String,
    /// Acting parties.
    #[prost(string, repeated, tag = "7")]
    acting_parties: Vec<String>,
    /// Consuming flag.
    #[prost(bool, tag = "9")]
    consuming: bool,
    /// Last descendant node id.
    #[prost(int32, tag = "11")]
    last_descendant_node_id: i32,
}

/// Ledger API identifier.
#[derive(Clone, PartialEq, Message)]
struct Identifier {
    /// Package id or package-name reference.
    #[prost(string, tag = "1")]
    package_id: String,
    /// Module name.
    #[prost(string, tag = "2")]
    module_name: String,
    /// Entity name.
    #[prost(string, tag = "3")]
    entity_name: String,
}
