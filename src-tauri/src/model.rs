//! Core data types shared across the engine, project DB, and AI layers.

use serde::{Deserialize, Serialize};

/// Translation status of a single unit. Ordered from least to most complete;
/// injection applies any unit at `Draft` or beyond.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Untranslated,
    Draft,
    Translated,
    Reviewed,
    Locked,
}

impl Status {
    /// Units at this level or higher have a translation worth injecting.
    pub fn is_applied(self) -> bool {
        !matches!(self, Status::Untranslated)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Status::Untranslated => "Untranslated",
            Status::Draft => "Draft",
            Status::Translated => "Translated",
            Status::Reviewed => "Reviewed",
            Status::Locked => "Locked",
        }
    }

    pub fn from_str(s: &str) -> Status {
        match s {
            "Draft" => Status::Draft,
            "Translated" => Status::Translated,
            "Reviewed" => Status::Reviewed,
            "Locked" => Status::Locked,
            _ => Status::Untranslated,
        }
    }
}

impl Default for Status {
    fn default() -> Self {
        Status::Untranslated
    }
}

/// What kind of text a unit holds. Drives UI grouping and AI prompt hints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnitKind {
    Dialogue,    // Show Text (401)
    ScrollText,  // Scrolling text (405)
    Choice,      // Show Choices option (102)
    Name,        // actor/item/skill name, change-name event
    Nickname,    // actor nickname
    Profile,     // actor profile
    Description, // item/skill description
    Message,     // stateN messageN
    Note,        // note box (may contain metadata)
    Term,        // System.json terms / type lists
    MapName,     // MapInfos name
    Title,       // game title
    Currency,    // currency unit
    Comment,     // event comment (108/408)
    PluginArg,   // plugin command argument (356/357)
    Script,      // raw script (355/655) — opt-in
    Other,
}

impl UnitKind {
    pub fn as_str(self) -> &'static str {
        match self {
            UnitKind::Dialogue => "Dialogue",
            UnitKind::ScrollText => "ScrollText",
            UnitKind::Choice => "Choice",
            UnitKind::Name => "Name",
            UnitKind::Nickname => "Nickname",
            UnitKind::Profile => "Profile",
            UnitKind::Description => "Description",
            UnitKind::Message => "Message",
            UnitKind::Note => "Note",
            UnitKind::Term => "Term",
            UnitKind::MapName => "MapName",
            UnitKind::Title => "Title",
            UnitKind::Currency => "Currency",
            UnitKind::Comment => "Comment",
            UnitKind::PluginArg => "PluginArg",
            UnitKind::Script => "Script",
            UnitKind::Other => "Other",
        }
    }

    pub fn from_str(s: &str) -> UnitKind {
        match s {
            "Dialogue" => UnitKind::Dialogue,
            "ScrollText" => UnitKind::ScrollText,
            "Choice" => UnitKind::Choice,
            "Name" => UnitKind::Name,
            "Nickname" => UnitKind::Nickname,
            "Profile" => UnitKind::Profile,
            "Description" => UnitKind::Description,
            "Message" => UnitKind::Message,
            "Note" => UnitKind::Note,
            "Term" => UnitKind::Term,
            "MapName" => UnitKind::MapName,
            "Title" => UnitKind::Title,
            "Currency" => UnitKind::Currency,
            "Comment" => UnitKind::Comment,
            "PluginArg" => UnitKind::PluginArg,
            "Script" => UnitKind::Script,
            _ => UnitKind::Other,
        }
    }
}

/// One translatable string located precisely inside a game file.
///
/// `pointer` is an RFC-6901 JSON Pointer into the parsed file, so injection
/// writes the translation back to the exact node without touching anything else.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransUnit {
    /// DB row id. 0 before the unit is persisted.
    pub id: i64,
    /// Relative file path from the data dir, e.g. "Map001.json".
    pub file: String,
    /// RFC-6901 JSON Pointer to the string node, e.g. "/events/3/pages/0/list/5/parameters/0".
    pub pointer: String,
    pub kind: UnitKind,
    /// Speaker, map name, or term key — helps AI and the human translator.
    pub context: Option<String>,
    /// Groups consecutive lines that form one logical message box (401/405).
    /// `None` means a standalone unit. UI merges units sharing a group key.
    pub group: Option<String>,
    pub source: String,
    pub translation: Option<String>,
    pub status: Status,
}

impl TransUnit {
    pub fn new(
        file: impl Into<String>,
        pointer: impl Into<String>,
        kind: UnitKind,
        source: impl Into<String>,
    ) -> Self {
        TransUnit {
            id: 0,
            file: file.into(),
            pointer: pointer.into(),
            kind,
            context: None,
            group: None,
            source: source.into(),
            translation: None,
            status: Status::Untranslated,
        }
    }

    pub fn with_context(mut self, ctx: Option<String>) -> Self {
        self.context = ctx;
        self
    }

    pub fn with_group(mut self, group: Option<String>) -> Self {
        self.group = group;
        self
    }
}
