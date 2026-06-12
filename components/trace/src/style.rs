//! Terminal styling helpers for trace output.

use std::io::{stderr, stdout, IsTerminal};

use owo_colors::OwoColorize;

/// Whether ANSI colour and emoji styling should be applied.
pub fn use_colour() -> bool {
    (stdout().is_terminal() || stderr().is_terminal()) && std::env::var_os("NO_COLOR").is_none()
}

/// Print a success message.
pub fn print_success(message: impl AsRef<str>) {
    println!("✅ {}", success(message.as_ref()));
}

/// Print an informational message.
pub fn print_info(message: impl AsRef<str>) {
    println!("ℹ️  {}", info(message.as_ref()));
}

/// Print a next-step hint.
pub fn print_hint(message: impl AsRef<str>) {
    println!("👉 {}", hint(message.as_ref()));
}

/// Print a warning message.
pub fn print_warning(message: impl AsRef<str>) {
    println!("⚠️  {}", warning(message.as_ref()));
}

/// Print an error message to stderr.
pub fn print_error(message: impl AsRef<str>) {
    eprintln!("❌ {}", error(message.as_ref()));
}

/// Section heading.
pub fn heading(text: &str) -> String {
    if use_colour() {
        text.bold().bright_cyan().to_string()
    } else {
        text.to_owned()
    }
}

/// Metadata field label.
pub fn label(text: &str) -> String {
    if use_colour() {
        text.bright_blue().to_string()
    } else {
        text.to_owned()
    }
}

/// Generic metadata or detail value.
pub fn value(text: &str) -> String {
    if use_colour() {
        text.white().to_string()
    } else {
        text.to_owned()
    }
}

/// Dimmed secondary value, such as a long contract id.
pub fn dim(text: &str) -> String {
    if use_colour() {
        text.dimmed().to_string()
    } else {
        text.to_owned()
    }
}

/// Template or record type name.
pub fn template(text: &str) -> String {
    if use_colour() {
        text.bold().bright_yellow().to_string()
    } else {
        text.to_owned()
    }
}

/// Party identifier.
pub fn party(text: &str) -> String {
    if use_colour() {
        text.bright_magenta().to_string()
    } else {
        text.to_owned()
    }
}

/// String literal in a Daml value.
pub fn string_literal(text: &str) -> String {
    if use_colour() {
        text.bright_green().to_string()
    } else {
        text.to_owned()
    }
}

/// Field name in a Daml record.
pub fn field_name(text: &str) -> String {
    if use_colour() {
        text.cyan().to_string()
    } else {
        text.to_owned()
    }
}

/// Success text.
pub fn success(text: &str) -> String {
    if use_colour() {
        text.bright_green().to_string()
    } else {
        text.to_owned()
    }
}

/// Informational text.
pub fn info(text: &str) -> String {
    if use_colour() {
        text.bright_cyan().to_string()
    } else {
        text.to_owned()
    }
}

/// Hint or command text.
pub fn hint(text: &str) -> String {
    if use_colour() {
        text.bright_yellow().to_string()
    } else {
        text.to_owned()
    }
}

/// Warning text.
pub fn warning(text: &str) -> String {
    if use_colour() {
        text.bright_yellow().to_string()
    } else {
        text.to_owned()
    }
}

/// Error text.
pub fn error(text: &str) -> String {
    if use_colour() {
        text.bold().bright_red().to_string()
    } else {
        text.to_owned()
    }
}

/// Profile name.
pub fn profile_name(text: &str) -> String {
    if use_colour() {
        text.bold().bright_magenta().to_string()
    } else {
        text.to_owned()
    }
}

/// File system path.
pub fn path(text: &str) -> String {
    dim(text)
}

/// Inline command suggestion.
pub fn command(text: &str) -> String {
    if use_colour() {
        text.bold().bright_green().to_string()
    } else {
        text.to_owned()
    }
}

/// Auth mode label.
pub fn auth_mode(mode: &str) -> String {
    if !use_colour() {
        return mode.to_owned();
    }

    match mode {
        "none" => mode.dimmed().to_string(),
        "localnet" => mode.bright_blue().to_string(),
        "remote" => mode.bright_magenta().to_string(),
        _ => value(mode),
    }
}

/// Tree connector glyph.
pub fn tree_branch(is_last: bool) -> String {
    let glyph = if is_last { "└─ " } else { "├─ " };
    if use_colour() {
        glyph.dimmed().to_string()
    } else {
        glyph.to_owned()
    }
}

/// Tree continuation prefix.
pub fn tree_prefix(is_last: bool) -> String {
    let glyph = if is_last { "   " } else { "│  " };
    if use_colour() {
        glyph.dimmed().to_string()
    } else {
        glyph.to_owned()
    }
}

/// Compact display for long identifiers.
pub fn compact_id(id: &str) -> String {
    if id.len() <= 48 {
        return dim(id);
    }

    let head = &id[..20];
    let tail = &id[id.len() - 12..];
    dim(&format!("{head}…{tail}"))
}

/// Colour one rendered Daml value line.
pub fn colour_daml_line(line: &str) -> String {
    if !use_colour() {
        return line.to_owned();
    }

    if let Some((left, right)) = line.split_once(" = ") {
        let field = left.trim_start();
        let indent = &left[..left.len() - field.len()];
        return format!(
            "{indent}{} = {}",
            field_name(field),
            colour_daml_value(right)
        );
    }

    if let Some((template_name, rest)) = line.split_once(" {") {
        return format!("{} {{{rest}", template(template_name));
    }

    if line.ends_with('}') || line == "{" || line == "}" {
        return dim(line);
    }

    colour_daml_value(line)
}

/// Colour a single-line Daml value fragment.
fn colour_daml_value(text: &str) -> String {
    if text.starts_with("party ") {
        return format!("party {}", party(&text[6..]));
    }
    if text.starts_with("contract ") {
        return format!("contract {}", compact_id(&text[9..]));
    }
    if text.starts_with('"') && text.ends_with('"') {
        return string_literal(text);
    }
    if text == "None" || text == "Some" || text == "()" {
        return dim(text);
    }

    value(text)
}
