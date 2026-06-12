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
    style,
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

/// Render a transaction summary with a nested event tree.
fn render_update(response: &GetUpdateResponse, parties: &[String]) -> Result<(), String> {
    let Some(get_update_response::Update::Transaction(transaction)) = &response.update else {
        return Err("GetUpdateById returned a non-transaction update".to_owned());
    };

    println!("{}", style::heading("📋 Transaction"));
    print_metadata_field("Update ID", &transaction.update_id);
    print_optional_metadata(
        "Record time",
        transaction.record_time.as_ref().map(format_timestamp),
    );
    print_optional_metadata(
        "Effective at",
        transaction.effective_at.as_ref().map(format_timestamp),
    );
    print_optional_metadata("Synchronizer", non_empty(&transaction.synchronizer_id));
    print_metadata_field("Offset", &transaction.offset.to_string());
    print_optional_metadata("Command ID", non_empty(&transaction.command_id));
    print_optional_metadata("Workflow ID", non_empty(&transaction.workflow_id));
    print_metadata_field("Visible as", &parties.join(", "));
    println!();
    println!("{}", style::heading("🌳 Event tree"));

    if transaction.events.is_empty() {
        println!("{}", style::dim("  (no visible events)"));
        return Ok(());
    }

    render_event_tree(&transaction.events);

    Ok(())
}

/// Print one required transaction metadata field.
fn print_metadata_field(label: &str, value: &str) {
    println!(
        "  {} : {}",
        style::label(&format!("{label:12}")),
        style_metadata_value(label, value)
    );
}

/// Print one optional transaction metadata field.
fn print_optional_metadata(label: &str, value: Option<String>) {
    if let Some(value) = value {
        print_metadata_field(label, &value);
    }
}

/// Style a metadata value based on its field label.
fn style_metadata_value(label: &str, value: &str) -> String {
    match label {
        "Visible as" => value
            .split(", ")
            .map(style::party)
            .collect::<Vec<_>>()
            .join(", "),
        "Update ID" | "Synchronizer" | "Command ID" | "Workflow ID" => style::dim(value),
        _ => style::value(value),
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

/// Render visible transaction events as a tree using exercise descendant bounds.
fn render_event_tree(events: &[Event]) {
    render_event_scope(events, 0, None, "");
}

/// Render sibling events until the optional node boundary is reached.
fn render_event_scope(
    events: &[Event],
    mut index: usize,
    max_node_id: Option<i32>,
    prefix: &str,
) -> usize {
    while index < events.len() && is_inside_scope(&events[index], max_node_id) {
        let next_index = event_subtree_next_index(events, index);
        let is_last =
            next_index >= events.len() || !is_inside_scope(&events[next_index], max_node_id);
        index = render_event_branch(events, index, prefix, is_last);
    }

    index
}

/// Render one event branch and return the next sibling index.
fn render_event_branch(events: &[Event], index: usize, prefix: &str, is_last: bool) -> usize {
    let event = &events[index];
    let branch = style::tree_branch(is_last);
    let child_prefix = format!("{prefix}{}", style::tree_prefix(is_last));

    println!("{prefix}{branch}{}", event_title(event));
    render_event_details(event, &child_prefix);

    if let Some(last_descendant_node_id) = event_last_descendant_node_id(event) {
        if last_descendant_node_id > event_node_id(event).unwrap_or_default() {
            return render_event_scope(
                events,
                index + 1,
                Some(last_descendant_node_id),
                &child_prefix,
            );
        }
    }

    index + 1
}

/// Return whether an event belongs to the current exercise subtree.
fn is_inside_scope(event: &Event, max_node_id: Option<i32>) -> bool {
    max_node_id.is_none_or(|max_node_id| {
        event_node_id(event).is_some_and(|node_id| node_id <= max_node_id)
    })
}

/// Return the index immediately after an event subtree.
fn event_subtree_next_index(events: &[Event], index: usize) -> usize {
    let Some(last_descendant_node_id) = event_last_descendant_node_id(&events[index]) else {
        return index + 1;
    };

    events[index + 1..]
        .iter()
        .position(|event| {
            event_node_id(event).is_some_and(|node_id| node_id > last_descendant_node_id)
        })
        .map_or(events.len(), |offset| index + offset + 1)
}

/// Render the event headline.
fn event_title(event: &Event) -> String {
    match &event.event {
        Some(event::Event::Created(created)) => format!(
            "{} #{} create {}",
            style::value("✨"),
            style::dim(&created.node_id.to_string()),
            style::template(&format_identifier(created.template_id.as_ref()))
        ),
        Some(event::Event::Archived(archived)) => format!(
            "{} #{} archive {}",
            style::value("📦"),
            style::dim(&archived.node_id.to_string()),
            style::template(&format_identifier(archived.template_id.as_ref()))
        ),
        Some(event::Event::Exercised(exercised)) => {
            let icon = if exercised.consuming { "🔥" } else { "⚡" };
            let consuming = if exercised.consuming {
                style::value("consuming")
            } else {
                style::dim("non-consuming")
            };
            format!(
                "{} #{} exercise {}.{} ({consuming})",
                style::value(icon),
                style::dim(&exercised.node_id.to_string()),
                style::template(&format_identifier(exercised.template_id.as_ref())),
                style::field_name(&exercised.choice)
            )
        }
        None => format!("{} {}", style::value("❓"), style::dim("#? unknown event")),
    }
}

/// Render details for one visible transaction event.
fn render_event_details(event: &Event, prefix: &str) {
    match &event.event {
        Some(event::Event::Created(created)) => {
            print_detail(prefix, "🔗 contract", &created.contract_id);
            print_party_list(prefix, "✍️  signatories", &created.signatories);
            print_party_list(prefix, "👀 observers", &created.observers);
            print_party_list(prefix, "👁️  witnesses", &created.witness_parties);
            if let Some(arguments) = &created.create_arguments {
                print_value_block(prefix, "📝 payload", &record_lines(arguments));
            }
        }
        Some(event::Event::Archived(archived)) => {
            print_detail(prefix, "🔗 contract", &archived.contract_id);
            print_party_list(prefix, "👁️  witnesses", &archived.witness_parties);
        }
        Some(event::Event::Exercised(exercised)) => {
            print_detail(prefix, "🔗 contract", &exercised.contract_id);
            print_party_list(prefix, "🎭 actors", &exercised.acting_parties);
            print_party_list(prefix, "👁️  witnesses", &exercised.witness_parties);
            if let Some(argument) = &exercised.choice_argument {
                print_value_block(prefix, "📥 argument", &value_lines(argument));
            }
            if let Some(result) = &exercised.exercise_result {
                print_value_block(prefix, "📤 result", &value_lines(result));
            }
        }
        None => {}
    }
}

/// Return the event node id, if the event shape is known.
fn event_node_id(event: &Event) -> Option<i32> {
    match &event.event {
        Some(event::Event::Created(created)) => Some(created.node_id),
        Some(event::Event::Archived(archived)) => Some(archived.node_id),
        Some(event::Event::Exercised(exercised)) => Some(exercised.node_id),
        None => None,
    }
}

/// Return the last descendant node id for exercise events.
fn event_last_descendant_node_id(event: &Event) -> Option<i32> {
    match &event.event {
        Some(event::Event::Exercised(exercised)) => Some(exercised.last_descendant_node_id),
        _ => None,
    }
}

/// Print one event detail.
fn print_detail(prefix: &str, label: &str, value: &str) {
    if value.is_empty() {
        return;
    }

    let rendered = style::compact_id(value);
    println!("{prefix}{}{rendered}", style::label(&format!("{label}: ")),);
}

/// Print a labelled list of parties when present.
fn print_party_list(prefix: &str, label: &str, parties: &[String]) {
    if parties.is_empty() {
        return;
    }

    let rendered = parties
        .iter()
        .map(|party| style::party(party))
        .collect::<Vec<_>>()
        .join(", ");
    println!("{prefix}{}{rendered}", style::label(&format!("{label}: ")),);
}

/// Print a multi-line Daml value block under one event detail.
fn print_value_block(prefix: &str, label: &str, lines: &[String]) {
    if lines.is_empty() {
        return;
    }

    println!("{prefix}{}:", style::label(label));
    for line in lines {
        println!("{prefix}  {}", style::colour_daml_line(line));
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

/// Render a Daml record as indented lines.
fn record_lines(record: &Record) -> Vec<String> {
    if record.fields.is_empty() {
        return vec![format!(
            "{} {{}}",
            format_identifier(record.record_id.as_ref())
        )];
    }

    let mut lines = vec![format!(
        "{} {{",
        format_identifier(record.record_id.as_ref())
    )];
    for field in &record.fields {
        append_labelled_value_lines(
            &mut lines,
            field.label.as_deref().unwrap_or("_"),
            &field.value,
        );
    }
    lines.push("}".to_owned());
    lines
}

/// Render a Daml value as indented lines.
fn value_lines(value: &Value) -> Vec<String> {
    match &value.sum {
        Some(value::Sum::Record(record)) => record_lines(record),
        Some(value::Sum::Variant(variant)) => {
            let Some(value) = &variant.value else {
                return vec![format!("{}()", variant.constructor)];
            };
            let child = value_lines(value);
            if child.len() == 1 {
                vec![format!("{}({})", variant.constructor, child[0])]
            } else {
                let mut lines = vec![format!("{}(", variant.constructor)];
                append_indented_lines(&mut lines, &child, 2);
                lines.push(")".to_owned());
                lines
            }
        }
        Some(value::Sum::Enum(enumeration)) => vec![enumeration.constructor.clone()],
        Some(value::Sum::List(list)) => list_lines(&list.elements),
        Some(value::Sum::Optional(optional)) => match &optional.value {
            Some(value) => {
                let child = value_lines(value);
                if child.len() == 1 {
                    vec![format!("Some({})", child[0])]
                } else {
                    let mut lines = vec!["Some(".to_owned()];
                    append_indented_lines(&mut lines, &child, 2);
                    lines.push(")".to_owned());
                    lines
                }
            }
            None => vec!["None".to_owned()],
        },
        Some(value::Sum::TextMap(map)) => text_map_lines(map),
        Some(value::Sum::GenMap(map)) => gen_map_lines(map),
        Some(value::Sum::ContractId(value)) => vec![format!("contract {value}")],
        Some(value::Sum::Int64(value)) => vec![value.to_string()],
        Some(value::Sum::Numeric(value)) => vec![value.clone()],
        Some(value::Sum::Text(value)) => vec![format!("{value:?}")],
        Some(value::Sum::Timestamp(micros)) => vec![format!("timestamp({micros}µs)")],
        Some(value::Sum::Party(value)) => vec![format!("party {value}")],
        Some(value::Sum::Bool(value)) => vec![value.to_string()],
        Some(value::Sum::Unit(_)) => vec!["()".to_owned()],
        Some(value::Sum::Date(value)) => vec![format!("date({value})")],
        None => vec!["<empty>".to_owned()],
    }
}

/// Append a field rendered as `label = value`.
fn append_labelled_value_lines(lines: &mut Vec<String>, label: &str, value: &Option<Value>) {
    let Some(value) = value else {
        lines.push(format!("  {label} = <empty>"));
        return;
    };
    let child = value_lines(value);
    if child.len() == 1 {
        lines.push(format!("  {label} = {}", child[0]));
    } else {
        lines.push(format!("  {label} ="));
        append_indented_lines(lines, &child, 4);
    }
}

/// Render a Daml list.
fn list_lines(elements: &[Value]) -> Vec<String> {
    if elements.is_empty() {
        return vec!["[]".to_owned()];
    }

    let mut lines = vec!["[".to_owned()];
    for element in elements {
        let child = value_lines(element);
        if child.len() == 1 {
            lines.push(format!("  - {}", child[0]));
        } else {
            lines.push("  -".to_owned());
            append_indented_lines(&mut lines, &child, 4);
        }
    }
    lines.push("]".to_owned());
    lines
}

/// Render a Daml text map.
fn text_map_lines(map: &TextMap) -> Vec<String> {
    if map.entries.is_empty() {
        return vec!["{}".to_owned()];
    }

    let mut lines = vec!["{".to_owned()];
    for entry in &map.entries {
        let child = entry
            .value
            .as_ref()
            .map(value_lines)
            .unwrap_or_else(|| vec!["<empty>".to_owned()]);
        if child.len() == 1 {
            lines.push(format!("  {:?} = {}", entry.key, child[0]));
        } else {
            lines.push(format!("  {:?} =", entry.key));
            append_indented_lines(&mut lines, &child, 4);
        }
    }
    lines.push("}".to_owned());
    lines
}

/// Render a Daml generic map.
fn gen_map_lines(map: &GenMap) -> Vec<String> {
    if map.entries.is_empty() {
        return vec!["{}".to_owned()];
    }

    let mut lines = vec!["{".to_owned()];
    for entry in &map.entries {
        let key = entry
            .key
            .as_ref()
            .map(value_lines)
            .unwrap_or_else(|| vec!["<empty>".to_owned()])
            .join(" ");
        let child = entry
            .value
            .as_ref()
            .map(value_lines)
            .unwrap_or_else(|| vec!["<empty>".to_owned()]);
        if child.len() == 1 {
            lines.push(format!("  {key} -> {}", child[0]));
        } else {
            lines.push(format!("  {key} ->"));
            append_indented_lines(&mut lines, &child, 4);
        }
    }
    lines.push("}".to_owned());
    lines
}

/// Append lines with a fixed number of leading spaces.
fn append_indented_lines(lines: &mut Vec<String>, child: &[String], spaces: usize) {
    let indent = " ".repeat(spaces);
    for line in child {
        lines.push(format!("{indent}{line}"));
    }
}

/// Render a protobuf timestamp as seconds plus nanoseconds.
fn format_timestamp(timestamp: &Timestamp) -> String {
    if timestamp.nanos == 0 {
        return style::value(&format!("{}s", timestamp.seconds));
    }

    let nanos = format!("{:09}", timestamp.nanos)
        .trim_end_matches('0')
        .to_owned();
    style::value(&format!("{}.{nanos}s", timestamp.seconds))
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
    /// Created contract payload.
    #[prost(message, optional, tag = "6")]
    create_arguments: Option<Record>,
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
    /// Interface id when the choice is inherited.
    #[prost(message, optional, tag = "5")]
    interface_id: Option<Identifier>,
    /// Choice name.
    #[prost(string, tag = "6")]
    choice: String,
    /// Choice argument.
    #[prost(message, optional, tag = "7")]
    choice_argument: Option<Value>,
    /// Acting parties.
    #[prost(string, repeated, tag = "8")]
    acting_parties: Vec<String>,
    /// Consuming flag.
    #[prost(bool, tag = "9")]
    consuming: bool,
    /// Witness parties.
    #[prost(string, repeated, tag = "10")]
    witness_parties: Vec<String>,
    /// Last descendant node id.
    #[prost(int32, tag = "11")]
    last_descendant_node_id: i32,
    /// Choice result.
    #[prost(message, optional, tag = "12")]
    exercise_result: Option<Value>,
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

/// Ledger API Daml value.
#[derive(Clone, PartialEq, Message)]
struct Value {
    /// Specific Daml value shape.
    #[prost(
        oneof = "value::Sum",
        tags = "1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16"
    )]
    sum: Option<value::Sum>,
}

/// google.protobuf.Empty stand-in for Daml unit values.
#[derive(Clone, PartialEq, Message)]
struct Empty {}

/// Oneof variants for Daml values.
mod value {
    use prost::Oneof;

    use super::{Empty, Enum, GenMap, List, Optional, Record, TextMap, Variant};

    /// Supported Daml value variants.
    #[derive(Clone, PartialEq, Oneof)]
    pub enum Sum {
        /// Unit value.
        #[prost(message, tag = "1")]
        Unit(Empty),
        /// Bool value.
        #[prost(bool, tag = "2")]
        Bool(bool),
        /// Int64 value.
        #[prost(sint64, tag = "3")]
        Int64(i64),
        /// Date value, encoded as days since the Unix epoch.
        #[prost(int32, tag = "4")]
        Date(i32),
        /// Timestamp value as microseconds since the Unix epoch.
        #[prost(sfixed64, tag = "5")]
        Timestamp(i64),
        /// Numeric value.
        #[prost(string, tag = "6")]
        Numeric(String),
        /// Party value.
        #[prost(string, tag = "7")]
        Party(String),
        /// Text value.
        #[prost(string, tag = "8")]
        Text(String),
        /// Contract id value.
        #[prost(string, tag = "9")]
        ContractId(String),
        /// Optional value.
        #[prost(message, tag = "10")]
        Optional(Optional),
        /// List value.
        #[prost(message, tag = "11")]
        List(List),
        /// Text map value.
        #[prost(message, tag = "12")]
        TextMap(TextMap),
        /// Generic map value.
        #[prost(message, tag = "13")]
        GenMap(GenMap),
        /// Record value.
        #[prost(message, tag = "14")]
        Record(Record),
        /// Variant value.
        #[prost(message, tag = "15")]
        Variant(Variant),
        /// Enum value.
        #[prost(message, tag = "16")]
        Enum(Enum),
    }
}

/// Ledger API Daml record.
#[derive(Clone, PartialEq, Message)]
struct Record {
    /// Optional record identifier.
    #[prost(message, optional, tag = "1")]
    record_id: Option<Identifier>,
    /// Record fields.
    #[prost(message, repeated, tag = "2")]
    fields: Vec<RecordField>,
}

/// Ledger API Daml record field.
#[derive(Clone, PartialEq, Message)]
struct RecordField {
    /// Optional field label.
    #[prost(string, optional, tag = "1")]
    label: Option<String>,
    /// Field value.
    #[prost(message, optional, tag = "2")]
    value: Option<Value>,
}

/// Ledger API Daml variant.
#[derive(Clone, PartialEq, Message)]
struct Variant {
    /// Optional variant identifier.
    #[prost(message, optional, tag = "1")]
    variant_id: Option<Identifier>,
    /// Variant constructor.
    #[prost(string, tag = "2")]
    constructor: String,
    /// Variant value.
    #[prost(message, optional, boxed, tag = "3")]
    value: Option<Box<Value>>,
}

/// Ledger API Daml enum.
#[derive(Clone, PartialEq, Message)]
struct Enum {
    /// Optional enum identifier.
    #[prost(message, optional, tag = "1")]
    enum_id: Option<Identifier>,
    /// Enum constructor.
    #[prost(string, tag = "2")]
    constructor: String,
}

/// Ledger API Daml list.
#[derive(Clone, PartialEq, Message)]
struct List {
    /// List elements.
    #[prost(message, repeated, tag = "1")]
    elements: Vec<Value>,
}

/// Ledger API Daml optional.
#[derive(Clone, PartialEq, Message)]
struct Optional {
    /// Present value, or absent for None.
    #[prost(message, optional, boxed, tag = "1")]
    value: Option<Box<Value>>,
}

/// Ledger API Daml text map.
#[derive(Clone, PartialEq, Message)]
struct TextMap {
    /// Text map entries.
    #[prost(message, repeated, tag = "1")]
    entries: Vec<TextMapEntry>,
}

/// Ledger API Daml text map entry.
#[derive(Clone, PartialEq, Message)]
struct TextMapEntry {
    /// Entry key.
    #[prost(string, tag = "1")]
    key: String,
    /// Entry value.
    #[prost(message, optional, tag = "2")]
    value: Option<Value>,
}

/// Ledger API Daml generic map.
#[derive(Clone, PartialEq, Message)]
struct GenMap {
    /// Generic map entries.
    #[prost(message, repeated, tag = "1")]
    entries: Vec<GenMapEntry>,
}

/// Ledger API Daml generic map entry.
#[derive(Clone, PartialEq, Message)]
struct GenMapEntry {
    /// Entry key.
    #[prost(message, optional, tag = "1")]
    key: Option<Value>,
    /// Entry value.
    #[prost(message, optional, tag = "2")]
    value: Option<Value>,
}
