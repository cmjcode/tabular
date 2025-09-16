#![allow(dead_code)]

// This module was copied from Lapce and adapted gradually for Tabular.
// It is currently behind a feature gate to avoid breaking the default build
// until all dependencies and types are fully wired.

// When porting pieces into use, remove this cfg and wire the types into the UI.

use std::{collections::{HashMap, HashSet}, rc::Rc, str::FromStr, sync::Arc, time::Duration};

// Many of these external crates are not yet in Cargo.toml or not used elsewhere in Tabular.
// Keep imports, but compilation is gated by the `lapce_editor` feature.
#[cfg(feature = "lapce_editor")]
mod impls {
    use super::*;
    use floem::{
        ViewId,
        action::{TimerToken, exec_after, show_context_menu},
        ext_event::create_ext_action,
        keyboard::Modifiers,
        kurbo::{Point, Rect, Vec2},
        menu::{Menu, MenuItem},
        pointer::{MouseButton, PointerInputEvent, PointerMoveEvent},
        prelude::SignalTrack,
        reactive::{
            ReadSignal, RwSignal, Scope, SignalGet, SignalUpdate, SignalWith, batch,
            use_context,
        },
        views::editor::{
            Editor,
            command::CommandExecuted,
            id::EditorId,
            movement,
            text::Document,
            view::{DiffSection, DiffSectionKind, LineInfo, ScreenLines, ScreenLinesBase},
            visual_line::{ConfigId, Lines, TextLayoutProvider, VLine, VLineInfo},
        },
    };
    use itertools::Itertools;
    use lapce_core::{
        buffer::{InvalLines, diff::DiffLines, rope_text::{RopeText, RopeTextVal}},
        command::{EditCommand, FocusCommand, MotionModeCommand, MultiSelectionCommand, ScrollCommand},
        cursor::{Cursor, CursorMode},
        editor::EditType,
        mode::{Mode, MotionMode},
        rope_text_pos::RopeTextPosition,
        selection::{InsertDrift, SelRegion, Selection},
    };
    use lapce_rpc::{buffer::BufferId, plugin::PluginId, proxy::ProxyResponse};
    use lapce_xi_rope::{Rope, RopeDelta, Transformer};
    use lsp_types::{
        CodeActionResponse, CompletionItem, CompletionTextEdit, GotoDefinitionResponse,
        HoverContents, InlayHint, InlayHintLabel, InlineCompletionTriggerKind, Location,
        MarkedString, MarkupKind, Range, TextEdit,
    };
    use nucleo::Utf32Str;
    use serde::{Deserialize, Serialize};

    // Re-export everything from the original file so downstream call-sites remain the same
    // after we rename this module file to `lapce_editor.rs`.
    pub use view::StickyHeaderInfo;

    pub mod diff;
    pub mod gutter;
    pub mod location;
    pub mod view;

    // Bring Tabular types; these may need deeper wiring later.
    use crate::{
        command::{CommandKind, InternalCommand, LapceCommand, LapceWorkbenchCommand},
        completion::CompletionStatus,
        config::LapceConfig,
        db::LapceDb,
        doc::{Doc, DocContent},
        editor_tab::EditorTabChild,
        id::{DiffEditorId, EditorTabId},
        inline_completion::{InlineCompletionItem, InlineCompletionStatus},
        keypress::{KeyPressFocus, condition::Condition},
        lsp::path_from_url,
        main_split::{Editors, MainSplitData, SplitDirection, SplitMoveDirection},
        markdown::{MarkdownContent, from_marked_string, from_plaintext, parse_markdown},
        panel::{
            call_hierarchy_view::CallHierarchyItemData,
            implementation_view::{init_implementation_root, map_to_location},
            kind::PanelKind,
        },
        snippet::Snippet,
        tracing::*,
        window_tab::{CommonData, Focus, WindowTabData},
    };

    // The entire original implementation would be pasted here.
    // For now, we keep the module skeleton compiling when the feature is enabled.
}

// Public facade: export key types only when the feature is active to avoid symbol leakage.
#[cfg(feature = "lapce_editor")]
pub use impls::*;
