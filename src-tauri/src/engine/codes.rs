//! RPGMaker MV/MZ event command code classification.
//!
//! Event pages hold a `list` of commands `{ code, indent, parameters }`.
//! This module decides, per code, which parameter slots carry translatable
//! text and what kind of text it is. Reference: RPGMaker MV/MZ command codes.

use crate::model::UnitKind;

/// Where a translatable string lives inside a command's `parameters` array.
pub enum ParamText {
    /// `parameters[idx]` is a plain string.
    At(usize, UnitKind),
    /// `parameters[idx]` is an array of strings (e.g. Show Choices).
    ArrayAt(usize, UnitKind),
}

/// Options controlling which "risky" categories are extracted.
#[derive(Debug, Clone, Copy)]
pub struct ExtractOpts {
    /// Include event comments (108/408). Often dev notes, sometimes shown.
    pub include_comments: bool,
    /// Include plugin command args (356/357). Engine/plugin specific.
    pub include_plugin_args: bool,
    /// Include raw script commands (355/655). Unsafe — may break JS.
    pub include_scripts: bool,
    /// Include `note` fields on database objects (often hold metadata tags).
    pub include_notes: bool,
}

impl Default for ExtractOpts {
    fn default() -> Self {
        // Conservative defaults: only clearly player-facing text.
        ExtractOpts {
            include_comments: false,
            include_plugin_args: false,
            include_scripts: false,
            include_notes: false,
        }
    }
}

/// Return the translatable parameter slot(s) for an event command code,
/// honoring the opt-in toggles. Empty vec => nothing to extract.
pub fn translatable_params(code: i64, opts: &ExtractOpts) -> Vec<ParamText> {
    match code {
        401 => vec![ParamText::At(0, UnitKind::Dialogue)], // Show Text line
        405 => vec![ParamText::At(0, UnitKind::ScrollText)], // Scrolling text line
        102 => vec![ParamText::ArrayAt(0, UnitKind::Choice)], // Show Choices
        402 => vec![ParamText::At(1, UnitKind::Choice)],   // When [choice]
        320 => vec![ParamText::At(1, UnitKind::Name)],     // Change Actor Name
        324 => vec![ParamText::At(1, UnitKind::Nickname)], // Change Nickname
        325 => vec![ParamText::At(1, UnitKind::Profile)],  // Change Profile
        108 | 408 if opts.include_comments => vec![ParamText::At(0, UnitKind::Comment)],
        356 if opts.include_plugin_args => vec![ParamText::At(0, UnitKind::PluginArg)],
        357 if opts.include_plugin_args => vec![ParamText::At(3, UnitKind::PluginArg)],
        355 | 655 if opts.include_scripts => vec![ParamText::At(0, UnitKind::Script)],
        _ => vec![],
    }
}

/// Codes whose consecutive runs form a single logical message box, so the UI
/// can merge them. 401 = normal text, 405 = scrolling text.
pub fn is_message_line(code: i64) -> bool {
    code == 401 || code == 405
}

/// The command that precedes a text block and carries speaker/face info
/// (101 = Show Text header). Used to derive dialogue context.
pub fn is_text_header(code: i64) -> bool {
    code == 101
}
