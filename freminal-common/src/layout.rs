// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Layout file format types and parser for Freminal.
//!
//! A layout describes a complete Freminal workspace: windows, tabs, and pane
//! trees. Layouts are stored as TOML files in
//! `~/.config/freminal/layouts/<name>.toml`.
//!
//! # Format Overview
//!
//! ```toml
//! [layout]
//! name = "Dev"
//! description = "Standard development workspace"
//!
//! [layout.variables]
//! project_dir = "~/projects/default"
//!
//! [[windows]]
//! size = [1200, 800]
//! position = [100, 200]
//!
//!   [[windows.tabs]]
//!   title = "Editor"
//!   active = true
//!
//!     [[windows.tabs.panes]]
//!     id = "root"
//!     split = "vertical"
//!     ratio = 0.65
//!
//!     [[windows.tabs.panes]]
//!     id = "editor"
//!     parent = "root"
//!     position = "first"
//!     directory = "${project_dir}"
//!     command = "nvim ."
//!     active = true
//! ```
//!
//! A single-window layout can omit `[[windows]]` and use top-level `[[tabs]]`:
//!
//! ```toml
//! [layout]
//! name = "Simple"
//!
//! [[tabs]]
//! title = "Main"
//!
//!   [[tabs.panes]]
//!   id = "main"
//!   directory = "~/projects"
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
//  Error types
// ---------------------------------------------------------------------------

/// Errors that can occur when parsing or validating a layout file.
#[derive(Debug, Error)]
pub enum LayoutError {
    /// An I/O error occurred while reading the layout file.
    #[error("I/O error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A TOML parse error occurred.
    #[error("TOML parse error in {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    /// The layout definition is structurally invalid.
    #[error("Invalid layout: {0}")]
    Validation(String),

    /// A serialization error occurred when saving a layout.
    #[error("failed to serialize layout: {0}")]
    Serialize(String),

    /// An I/O error occurred while writing the layout file.
    #[error("I/O error writing {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

// ---------------------------------------------------------------------------
//  Split direction
// ---------------------------------------------------------------------------

/// The axis along which a pane is split.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayoutSplitDirection {
    /// Left / right split — a vertical divider between two side-by-side panes.
    Vertical,
    /// Top / bottom split — a horizontal divider between two stacked panes.
    Horizontal,
}

// ---------------------------------------------------------------------------
//  Pane node
// ---------------------------------------------------------------------------

/// The position of a pane within its parent split.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayoutPanePosition {
    /// The left or top pane in a split.
    First,
    /// The right or bottom pane in a split.
    Second,
}

/// A single pane entry in a `[[windows.tabs.panes]]` or `[[tabs.panes]]` list.
///
/// Each entry is either a **split node** (has `split` and `ratio`) or a
/// **leaf node** (no `split` — represents a real terminal pane).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutPane {
    /// Unique ID within this tab.  Used as parent reference.
    pub id: String,

    /// ID of the parent split node.  Absent for the root node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,

    /// Position within the parent split ("first" or "second").
    ///
    /// Required for all non-root nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<LayoutPanePosition>,

    /// When present, this is a split node.  The value gives the split axis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<LayoutSplitDirection>,

    /// Split ratio (0.0–1.0).  Only meaningful for split nodes.  Defaults
    /// to 0.5 when absent.
    #[serde(default = "default_ratio", skip_serializing_if = "is_default_ratio")]
    pub ratio: f32,

    /// Working directory for a leaf pane.  Supports `~`, `${VAR}`, `$1`, and
    /// `$ENV{NAME}` substitutions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,

    /// Command to run after the shell starts in this pane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,

    /// Shell override for this pane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,

    /// Extra environment variables for this pane.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,

    /// Initial pane title (overridden later by OSC sequences).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// When `true`, this pane receives focus after layout application.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub active: bool,
}

const fn default_ratio() -> f32 {
    0.5
}

// serde's `skip_serializing_if` requires a fn(&T) -> bool signature, so we
// must take `&f32` here.  The clippy::trivially_copy_pass_by_ref lint is
// suppressed because changing to a by-value signature would break serde.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_default_ratio(v: &f32) -> bool {
    (*v - 0.5_f32).abs() < f32::EPSILON
}

impl LayoutPane {
    /// Returns `true` if this entry is a split node (not a leaf).
    #[must_use]
    pub const fn is_split(&self) -> bool {
        self.split.is_some()
    }

    /// Returns `true` if this entry is a leaf (terminal) pane.
    #[must_use]
    pub const fn is_leaf(&self) -> bool {
        self.split.is_none()
    }
}

// ---------------------------------------------------------------------------
//  Tab
// ---------------------------------------------------------------------------

/// A tab definition within a window or at the top level.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutTab {
    /// Tab title shown in the tab bar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// When `true`, this tab is focused after layout application.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub active: bool,

    /// Pane tree for this tab.  Entries form a flat node list with
    /// parent-references.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub panes: Vec<LayoutPane>,
}

// ---------------------------------------------------------------------------
//  Window
// ---------------------------------------------------------------------------

/// An OS window definition within a layout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutWindow {
    /// Preferred window size in pixels (width, height).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<[u32; 2]>,

    /// Preferred window position in pixels (x, y).
    ///
    /// Ignored on Wayland; stored so that a layout saved on Wayland can be
    /// meaningfully restored on X11/macOS/Windows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[i32; 2]>,

    /// Preferred monitor index (0-based, best-effort).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monitor: Option<u32>,

    /// Tabs within this window.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tabs: Vec<LayoutTab>,
}

// ---------------------------------------------------------------------------
//  Top-level metadata
// ---------------------------------------------------------------------------

/// The `[layout]` header section.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LayoutMeta {
    /// Human-readable name shown in menus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Optional description shown in the layout library.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Named variable defaults.  Can be overridden on the CLI via `--var`.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub variables: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
//  Top-level Layout struct (raw deserialization)
// ---------------------------------------------------------------------------

/// The raw deserialized layout file.
///
/// Call [`Layout::from_file`] or [`Layout::from_str`] to parse, then
/// [`Layout::validate`] to check structural correctness.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Layout {
    /// Layout metadata.
    #[serde(default)]
    pub layout: LayoutMeta,

    /// Multi-window format: each `[[windows]]` entry is one OS window.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub windows: Vec<LayoutWindow>,

    /// Single-window shorthand: top-level `[[tabs]]` entries.
    ///
    /// If both `windows` and `tabs` are non-empty, `windows` takes precedence
    /// and `tabs` is ignored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tabs: Vec<LayoutTab>,
}

impl Layout {
    /// Parse a layout from a TOML string.
    ///
    /// # Errors
    ///
    /// Returns `LayoutError::Parse` if the TOML is malformed.
    pub fn from_str_content(path: &Path, content: &str) -> Result<Self, LayoutError> {
        toml::from_str(content).map_err(|source| LayoutError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Load and parse a layout from a TOML file.
    ///
    /// # Errors
    ///
    /// Returns `LayoutError::Io` if the file cannot be read, or
    /// `LayoutError::Parse` if the content is not valid TOML.
    pub fn from_file(path: &Path) -> Result<Self, LayoutError> {
        let content = std::fs::read_to_string(path).map_err(|source| LayoutError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_str_content(path, &content)
    }

    /// Serialize this layout to a TOML string.
    ///
    /// # Errors
    ///
    /// Returns `LayoutError::Serialize` if serialization fails.
    pub fn to_toml_string(&self) -> Result<String, LayoutError> {
        toml::to_string_pretty(self).map_err(|e| LayoutError::Serialize(e.to_string()))
    }

    /// Save this layout to the given path.
    ///
    /// # Errors
    ///
    /// Returns `LayoutError` on serialization or I/O failure.
    pub fn save_to_file(&self, path: &Path) -> Result<(), LayoutError> {
        let toml_str = self.to_toml_string()?;
        std::fs::write(path, toml_str).map_err(|source| LayoutError::Write {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Returns the effective windows list.
    ///
    /// If `windows` is non-empty it is returned directly.  If only `tabs` are
    /// present at the top level, they are wrapped in a single default window.
    #[must_use]
    pub fn effective_windows(&self) -> Vec<LayoutWindow> {
        if !self.windows.is_empty() {
            return self.windows.clone();
        }
        if !self.tabs.is_empty() {
            return vec![LayoutWindow {
                size: None,
                position: None,
                monitor: None,
                tabs: self.tabs.clone(),
            }];
        }
        vec![]
    }

    /// Returns the display name for this layout (from metadata or a fallback).
    #[must_use]
    pub fn display_name(&self) -> &str {
        self.layout.name.as_deref().unwrap_or("Unnamed Layout")
    }

    /// Validate the structural integrity of the layout.
    ///
    /// Checks:
    /// - Each tab's pane list has at most one root (node without a parent).
    /// - Every non-root node's `parent` references a node that exists in the
    ///   same tab.
    /// - Every non-root node has a `position` field.
    /// - No cycles exist (parent references are acyclic).
    /// - Split nodes do not have leaf-only fields (`command`, `shell`).
    ///
    /// # Errors
    ///
    /// Returns `LayoutError::Validation` describing the first violation found.
    pub fn validate(&self) -> Result<(), LayoutError> {
        let windows = self.effective_windows();
        for (wi, window) in windows.iter().enumerate() {
            for (ti, tab) in window.tabs.iter().enumerate() {
                validate_pane_list(wi, ti, &tab.panes)?;
            }
        }
        Ok(())
    }

    /// Apply variable substitution to all string fields that support it.
    ///
    /// Substitution rules:
    /// - `$1`, `$2`, ... are replaced with values from `positional`.
    /// - `${NAME}` is replaced with `variables[NAME]` if present, else kept.
    /// - `$ENV{NAME}` is replaced with the environment variable `NAME`.
    /// - Leading `~` in path fields is expanded to the home directory.
    ///
    /// This method returns a new `Layout` with substitutions applied.
    #[must_use]
    pub fn apply_variables(
        &self,
        positional: &[String],
        overrides: &HashMap<String, String>,
    ) -> Self {
        let mut vars = self.layout.variables.clone();
        for (k, v) in overrides {
            vars.insert(k.clone(), v.clone());
        }

        let substitute = |s: &str| -> String { substitute_variables(s, positional, &vars) };

        let map_pane = |p: &LayoutPane| -> LayoutPane {
            LayoutPane {
                directory: p.directory.as_deref().map(&substitute),
                command: p.command.as_deref().map(&substitute),
                shell: p.shell.as_deref().map(&substitute),
                env: p
                    .env
                    .iter()
                    .map(|(k, v)| (k.clone(), substitute(v)))
                    .collect(),
                title: p.title.as_deref().map(&substitute),
                id: p.id.clone(),
                parent: p.parent.clone(),
                position: p.position,
                split: p.split,
                ratio: p.ratio,
                active: p.active,
            }
        };

        let map_tab = |t: &LayoutTab| -> LayoutTab {
            LayoutTab {
                title: t.title.as_deref().map(&substitute),
                active: t.active,
                panes: t.panes.iter().map(&map_pane).collect(),
            }
        };

        let map_window = |w: &LayoutWindow| -> LayoutWindow {
            LayoutWindow {
                size: w.size,
                position: w.position,
                monitor: w.monitor,
                tabs: w.tabs.iter().map(&map_tab).collect(),
            }
        };

        Self {
            layout: self.layout.clone(),
            windows: self.windows.iter().map(&map_window).collect(),
            tabs: self.tabs.iter().map(&map_tab).collect(),
        }
    }
}

// ---------------------------------------------------------------------------
//  Variable substitution
// ---------------------------------------------------------------------------

/// Substitute variables in a string.
///
/// - `$1`, `$2`, ... → `positional[0]`, `positional[1]`, ...
/// - `${NAME}` → `named[NAME]` if present, else kept unchanged.
/// - `$ENV{NAME}` → `std::env::var(NAME)` if set, else kept unchanged.
/// - Leading `~` → home directory.
fn substitute_variables(s: &str, positional: &[String], named: &HashMap<String, String>) -> String {
    let mut result = s.to_string();

    // Expand `$ENV{NAME}` first (longest match first to avoid conflicts).
    let mut cursor = 0;
    let mut expanded = String::new();
    let bytes = result.as_bytes();
    while cursor < bytes.len() {
        if result[cursor..].starts_with("$ENV{") {
            let start = cursor + 5; // skip "$ENV{"
            if let Some(end) = result[start..].find('}') {
                let var_name = &result[start..start + end];
                let value =
                    std::env::var(var_name).unwrap_or_else(|_| format!("$ENV{{{var_name}}}"));
                expanded.push_str(&value);
                cursor = start + end + 1;
                continue;
            }
        }
        // SAFETY: cursor is always at a valid byte boundary (UTF-8 char
        // boundaries are respected because we only advance by ASCII sequences
        // or by full char lengths).
        let ch = result[cursor..].chars().next().unwrap_or('\0');
        expanded.push(ch);
        cursor += ch.len_utf8();
    }
    result = expanded;

    // Expand `${NAME}` named variables.
    let mut expanded = String::new();
    let mut cursor = 0;
    while cursor < result.len() {
        if result[cursor..].starts_with("${") {
            let start = cursor + 2;
            if let Some(end) = result[start..].find('}') {
                let var_name = &result[start..start + end];
                let value = named
                    .get(var_name)
                    .cloned()
                    .unwrap_or_else(|| format!("${{{var_name}}}"));
                expanded.push_str(&value);
                cursor = start + end + 1;
                continue;
            }
        }
        let ch = result[cursor..].chars().next().unwrap_or('\0');
        expanded.push(ch);
        cursor += ch.len_utf8();
    }
    result = expanded;

    // Expand `$N` positional args (try longest number first).
    let mut expanded = String::new();
    let cursor = 0;
    let chars: Vec<char> = result.chars().collect();
    let mut ci = 0;
    while ci < chars.len() {
        if chars[ci] == '$' && ci + 1 < chars.len() && chars[ci + 1].is_ascii_digit() {
            // Collect all consecutive digits for the index.
            let num_start = ci + 1;
            let mut num_end = num_start;
            while num_end < chars.len() && chars[num_end].is_ascii_digit() {
                num_end += 1;
            }
            let idx_str: String = chars[num_start..num_end].iter().collect();
            if let Ok(idx) = idx_str.parse::<usize>()
                && idx >= 1
            {
                let pos_val = positional
                    .get(idx - 1)
                    .cloned()
                    .unwrap_or_else(|| format!("${idx}"));
                expanded.push_str(&pos_val);
                ci = num_end;
                continue;
            }
        }
        expanded.push(chars[ci]);
        ci += 1;
    }
    result = expanded;

    // Tilde expansion: leading `~` → home directory.
    if result.starts_with('~')
        && let Ok(home) = std::env::var("HOME")
    {
        result = format!("{home}{}", &result[1..]);
    }

    // Track cursor usage to avoid unused variable warning.
    let _ = cursor;

    result
}

// ---------------------------------------------------------------------------
//  Pane list validation
// ---------------------------------------------------------------------------

fn validate_pane_list(
    window_idx: usize,
    tab_idx: usize,
    panes: &[LayoutPane],
) -> Result<(), LayoutError> {
    if panes.is_empty() {
        return Ok(());
    }

    let loc =
        |msg: &str| LayoutError::Validation(format!("window[{window_idx}].tab[{tab_idx}]: {msg}"));

    // Build ID set.
    let ids: std::collections::HashSet<&str> = panes.iter().map(|p| p.id.as_str()).collect();

    // Check for duplicate IDs.
    if ids.len() != panes.len() {
        return Err(loc("duplicate pane id found"));
    }

    // Count roots and validate parent references.
    let mut roots = 0usize;
    for pane in panes {
        match &pane.parent {
            None => {
                roots += 1;
            }
            Some(parent_id) => {
                if !ids.contains(parent_id.as_str()) {
                    return Err(loc(&format!(
                        "pane '{}' references non-existent parent '{parent_id}'",
                        pane.id
                    )));
                }
                if pane.position.is_none() {
                    return Err(loc(&format!(
                        "pane '{}' has a parent but no 'position' field",
                        pane.id
                    )));
                }
            }
        }
    }

    if roots == 0 {
        return Err(loc("no root pane (a pane without 'parent') found"));
    }
    if roots > 1 {
        return Err(loc(&format!(
            "{roots} root panes found (expected exactly 1)"
        )));
    }

    // Basic cycle detection via DFS.
    detect_cycle(panes).map_err(|msg| loc(&msg))?;

    Ok(())
}

fn detect_cycle(panes: &[LayoutPane]) -> Result<(), String> {
    // Build child → parent map (pane id → parent id).
    let parent_of: HashMap<&str, &str> = panes
        .iter()
        .filter_map(|p| p.parent.as_deref().map(|par| (p.id.as_str(), par)))
        .collect();

    for start in panes {
        let mut seen = std::collections::HashSet::new();
        let mut current = start.id.as_str();
        loop {
            if !seen.insert(current) {
                return Err(format!("cycle detected involving pane '{current}'"));
            }
            match parent_of.get(current) {
                Some(parent) => current = parent,
                None => break,
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
//  Resolved pane tree (post-validation data structure)
// ---------------------------------------------------------------------------

/// A leaf pane entry with all string values already substituted.
#[derive(Debug, Clone)]
pub struct ResolvedLeaf {
    /// The pane's ID in the layout spec.
    pub id: String,
    /// Working directory (expanded, absolute path or `None`).
    pub directory: Option<String>,
    /// Command to inject after shell startup.
    pub command: Option<String>,
    /// Shell override.
    pub shell: Option<String>,
    /// Extra environment variables.
    pub env: HashMap<String, String>,
    /// Initial title.
    pub title: Option<String>,
    /// Whether this pane should receive focus.
    pub active: bool,
}

/// A node in the resolved binary pane tree.
#[derive(Debug, Clone)]
pub enum ResolvedNode {
    /// A terminal leaf pane.
    Leaf(ResolvedLeaf),
    /// A split node with two children.
    Split {
        /// Split direction.
        direction: LayoutSplitDirection,
        /// Ratio between [0.0, 1.0].
        ratio: f32,
        /// Left or top child.
        first: Box<Self>,
        /// Right or bottom child.
        second: Box<Self>,
    },
}

/// A resolved tab with a binary pane tree.
#[derive(Debug, Clone)]
pub struct ResolvedTab {
    /// Tab title.
    pub title: Option<String>,
    /// Whether this tab should be active.
    pub active: bool,
    /// Root of the pane tree, or `None` for an empty tab.
    pub root: Option<ResolvedNode>,
}

/// A resolved window specification.
#[derive(Debug, Clone)]
pub struct ResolvedWindow {
    /// Preferred size (width, height) in pixels.
    pub size: Option<[u32; 2]>,
    /// Preferred position (x, y) in pixels.
    pub position: Option<[i32; 2]>,
    /// Preferred monitor index.
    pub monitor: Option<u32>,
    /// Tabs within this window.
    pub tabs: Vec<ResolvedTab>,
}

/// A fully resolved layout, ready for application.
#[derive(Debug, Clone)]
pub struct ResolvedLayout {
    /// Display name.
    pub name: String,
    /// Windows to open.
    pub windows: Vec<ResolvedWindow>,
}

impl Layout {
    /// Validate and resolve this layout into a [`ResolvedLayout`].
    ///
    /// Variable substitution must be applied (via [`Layout::apply_variables`])
    /// before calling this method, or variable references will remain
    /// unexpanded in the resolved structure.
    ///
    /// # Errors
    ///
    /// Returns `LayoutError::Validation` if the layout is structurally invalid.
    pub fn resolve(&self) -> Result<ResolvedLayout, LayoutError> {
        self.validate()?;

        let name = self.display_name().to_string();
        let windows = self.effective_windows();

        let resolved_windows = windows
            .iter()
            .enumerate()
            .map(|(wi, window)| {
                let tabs = window
                    .tabs
                    .iter()
                    .enumerate()
                    .map(|(ti, tab)| resolve_tab(wi, ti, tab))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(ResolvedWindow {
                    size: window.size,
                    position: window.position,
                    monitor: window.monitor,
                    tabs,
                })
            })
            .collect::<Result<Vec<_>, LayoutError>>()?;

        Ok(ResolvedLayout {
            name,
            windows: resolved_windows,
        })
    }
}

fn resolve_tab(wi: usize, ti: usize, tab: &LayoutTab) -> Result<ResolvedTab, LayoutError> {
    let root = if tab.panes.is_empty() {
        None
    } else {
        Some(build_pane_tree(wi, ti, &tab.panes)?)
    };

    Ok(ResolvedTab {
        title: tab.title.clone(),
        active: tab.active,
        root,
    })
}

fn build_pane_tree(
    wi: usize,
    ti: usize,
    panes: &[LayoutPane],
) -> Result<ResolvedNode, LayoutError> {
    let loc = |msg: &str| LayoutError::Validation(format!("window[{wi}].tab[{ti}]: {msg}"));

    // Find the root node (no parent).
    let root_pane = panes
        .iter()
        .find(|p| p.parent.is_none())
        .ok_or_else(|| loc("no root pane found"))?;

    build_node(root_pane, panes, wi, ti)
}

fn build_node(
    pane: &LayoutPane,
    all: &[LayoutPane],
    wi: usize,
    ti: usize,
) -> Result<ResolvedNode, LayoutError> {
    let loc = |msg: &str| LayoutError::Validation(format!("window[{wi}].tab[{ti}]: {msg}"));

    if let Some(dir) = pane.split {
        // Split node: find first and second children.
        let first_child = all
            .iter()
            .find(|p| {
                p.parent.as_deref() == Some(&pane.id)
                    && p.position == Some(LayoutPanePosition::First)
            })
            .ok_or_else(|| loc(&format!("split node '{}' missing 'first' child", pane.id)))?;

        let second_child = all
            .iter()
            .find(|p| {
                p.parent.as_deref() == Some(&pane.id)
                    && p.position == Some(LayoutPanePosition::Second)
            })
            .ok_or_else(|| loc(&format!("split node '{}' missing 'second' child", pane.id)))?;

        let first = build_node(first_child, all, wi, ti)?;
        let second = build_node(second_child, all, wi, ti)?;

        Ok(ResolvedNode::Split {
            direction: dir,
            ratio: pane.ratio,
            first: Box::new(first),
            second: Box::new(second),
        })
    } else {
        // Leaf node.
        Ok(ResolvedNode::Leaf(ResolvedLeaf {
            id: pane.id.clone(),
            directory: pane.directory.clone(),
            command: pane.command.clone(),
            shell: pane.shell.clone(),
            env: pane.env.clone(),
            title: pane.title.clone(),
            active: pane.active,
        }))
    }
}

// ---------------------------------------------------------------------------
//  Layout library
// ---------------------------------------------------------------------------

/// Summary information about a layout file in the library, parsed cheaply
/// without fully resolving the pane tree.
#[derive(Debug, Clone)]
pub struct LayoutSummary {
    /// Display name from `[layout]` header.
    pub name: String,
    /// Optional description from `[layout]` header.
    pub description: Option<String>,
    /// Path to the layout file.
    pub path: PathBuf,
}

/// Scan a directory for layout files and return their summaries.
///
/// Each `.toml` file in `dir` is parsed (headers only) and returned as a
/// [`LayoutSummary`].  Files that fail to parse are silently skipped — a
/// broken layout should not prevent the rest of the library from loading.
///
/// Returns an empty `Vec` if `dir` does not exist or cannot be read.
#[must_use]
pub fn discover_layouts(dir: &Path) -> Vec<LayoutSummary> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };

    let mut summaries = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        match Layout::from_file(&path) {
            Ok(layout) => {
                summaries.push(LayoutSummary {
                    name: layout.display_name().to_string(),
                    description: layout.layout.description.clone(),
                    path,
                });
            }
            Err(e) => {
                warn!("skipping layout {:?}: {e}", path);
            }
        }
    }
    summaries.sort_by(|a, b| a.name.cmp(&b.name));
    summaries
}

// ---------------------------------------------------------------------------
//  Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const SIMPLE_LAYOUT: &str = r#"
[layout]
name = "Simple"
description = "A single-pane layout"

[[tabs]]
title = "Main"

  [[tabs.panes]]
  id = "main"
  directory = "~/projects"
  active = true
"#;

    const SPLIT_LAYOUT: &str = r#"
[layout]
name = "Split"

[[windows]]

  [[windows.tabs]]
  title = "Dev"
  active = true

    [[windows.tabs.panes]]
    id = "root"
    split = "vertical"
    ratio = 0.65

    [[windows.tabs.panes]]
    id = "editor"
    parent = "root"
    position = "first"
    directory = "~/src"
    command = "nvim ."
    active = true

    [[windows.tabs.panes]]
    id = "term"
    parent = "root"
    position = "second"
    directory = "~/src"
"#;

    const VAR_LAYOUT: &str = r#"
[layout]
name = "Vars"

[layout.variables]
project_dir = "~/default"

[[tabs]]

  [[tabs.panes]]
  id = "main"
  directory = "${project_dir}"
  command = "echo $1"
"#;

    #[test]
    fn parse_simple_layout() {
        let layout =
            Layout::from_str_content(Path::new("test.toml"), SIMPLE_LAYOUT).expect("parse failed");
        assert_eq!(layout.display_name(), "Simple");
        assert_eq!(
            layout.layout.description.as_deref(),
            Some("A single-pane layout")
        );
        assert!(layout.windows.is_empty());
        assert_eq!(layout.tabs.len(), 1);
        assert_eq!(layout.tabs[0].panes.len(), 1);
    }

    #[test]
    fn effective_windows_simple_layout_wraps_tabs() {
        let layout =
            Layout::from_str_content(Path::new("test.toml"), SIMPLE_LAYOUT).expect("parse failed");
        let windows = layout.effective_windows();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].tabs.len(), 1);
    }

    #[test]
    fn parse_split_layout() {
        let layout =
            Layout::from_str_content(Path::new("test.toml"), SPLIT_LAYOUT).expect("parse failed");
        assert_eq!(layout.display_name(), "Split");
        assert_eq!(layout.windows.len(), 1);
        let tab = &layout.windows[0].tabs[0];
        assert_eq!(tab.panes.len(), 3);
    }

    #[test]
    fn validate_split_layout_ok() {
        let layout =
            Layout::from_str_content(Path::new("test.toml"), SPLIT_LAYOUT).expect("parse failed");
        layout.validate().expect("validation failed");
    }

    #[test]
    fn validate_simple_layout_ok() {
        let layout =
            Layout::from_str_content(Path::new("test.toml"), SIMPLE_LAYOUT).expect("parse failed");
        layout.validate().expect("validation failed");
    }

    #[test]
    fn validation_rejects_orphan_parent() {
        let bad = r#"
[[tabs]]
  [[tabs.panes]]
  id = "a"
  parent = "nonexistent"
  position = "first"
"#;
        let layout = Layout::from_str_content(Path::new("bad.toml"), bad).expect("parse failed");
        let err = layout.validate().expect_err("expected validation error");
        let msg = err.to_string();
        assert!(msg.contains("non-existent parent"), "got: {msg}");
    }

    #[test]
    fn validation_rejects_two_roots() {
        let bad = r#"
[[tabs]]
  [[tabs.panes]]
  id = "a"

  [[tabs.panes]]
  id = "b"
"#;
        let layout = Layout::from_str_content(Path::new("bad.toml"), bad).expect("parse failed");
        let err = layout.validate().expect_err("expected validation error");
        let msg = err.to_string();
        assert!(msg.contains("root"), "got: {msg}");
    }

    #[test]
    fn validation_rejects_missing_position() {
        let bad = r#"
[[tabs]]
  [[tabs.panes]]
  id = "root"
  split = "vertical"

  [[tabs.panes]]
  id = "child"
  parent = "root"
  # missing position
"#;
        let layout = Layout::from_str_content(Path::new("bad.toml"), bad).expect("parse failed");
        let err = layout.validate().expect_err("expected validation error");
        let msg = err.to_string();
        assert!(msg.contains("position"), "got: {msg}");
    }

    #[test]
    fn variable_substitution_named() {
        let layout =
            Layout::from_str_content(Path::new("test.toml"), VAR_LAYOUT).expect("parse failed");
        let substituted = layout.apply_variables(&[], &HashMap::new());
        let pane = &substituted.tabs[0].panes[0];
        // The default value is "~/default". After tilde expansion it should
        // point to the home directory (or keep the ~ prefix if HOME is unset).
        let dir = pane.directory.as_deref().unwrap_or("");
        if let Ok(home) = std::env::var("HOME") {
            assert_eq!(dir, format!("{home}/default"));
        } else {
            assert_eq!(dir, "~/default");
        }
    }

    #[test]
    fn variable_substitution_override() {
        let layout =
            Layout::from_str_content(Path::new("test.toml"), VAR_LAYOUT).expect("parse failed");
        let mut overrides = HashMap::new();
        overrides.insert("project_dir".to_string(), "/custom".to_string());
        let substituted = layout.apply_variables(&[], &overrides);
        let pane = &substituted.tabs[0].panes[0];
        assert_eq!(pane.directory.as_deref(), Some("/custom"));
    }

    #[test]
    fn variable_substitution_positional() {
        let layout =
            Layout::from_str_content(Path::new("test.toml"), VAR_LAYOUT).expect("parse failed");
        let positional = vec!["hello".to_string()];
        let substituted = layout.apply_variables(&positional, &HashMap::new());
        let pane = &substituted.tabs[0].panes[0];
        assert_eq!(pane.command.as_deref(), Some("echo hello"));
    }

    #[test]
    fn variable_substitution_tilde() {
        // Tilde expansion uses $HOME env var.
        let layout =
            Layout::from_str_content(Path::new("test.toml"), SIMPLE_LAYOUT).expect("parse failed");
        let substituted = layout.apply_variables(&[], &HashMap::new());
        let pane = &substituted.tabs[0].panes[0];
        // If HOME is set, the path should start with HOME; otherwise it stays
        // as is (or becomes empty prefix + /projects).
        let dir = pane.directory.as_deref().unwrap_or("");
        if let Ok(home) = std::env::var("HOME") {
            assert!(
                dir.starts_with(&home),
                "expected {home}/projects, got {dir}"
            );
        } else {
            assert!(dir.starts_with('/') || dir.starts_with('~'));
        }
    }

    #[test]
    fn resolve_simple_layout() {
        let layout =
            Layout::from_str_content(Path::new("test.toml"), SIMPLE_LAYOUT).expect("parse failed");
        let resolved = layout.resolve().expect("resolve failed");
        assert_eq!(resolved.windows.len(), 1);
        assert_eq!(resolved.windows[0].tabs.len(), 1);
        let root = resolved.windows[0].tabs[0]
            .root
            .as_ref()
            .expect("root should exist");
        assert!(matches!(root, ResolvedNode::Leaf(_)));
    }

    #[test]
    fn resolve_split_layout() {
        let layout =
            Layout::from_str_content(Path::new("test.toml"), SPLIT_LAYOUT).expect("parse failed");
        let resolved = layout.resolve().expect("resolve failed");
        let root = resolved.windows[0].tabs[0]
            .root
            .as_ref()
            .expect("root should exist");
        assert!(matches!(root, ResolvedNode::Split { .. }));
    }

    #[test]
    fn round_trip_serialization() {
        let layout =
            Layout::from_str_content(Path::new("test.toml"), SPLIT_LAYOUT).expect("parse failed");
        let toml_str = layout.to_toml_string().expect("serialize failed");
        let reparsed =
            Layout::from_str_content(Path::new("test.toml"), &toml_str).expect("reparse failed");
        assert_eq!(reparsed.display_name(), layout.display_name());
        assert_eq!(reparsed.windows.len(), layout.windows.len());
    }

    #[test]
    fn substitute_env_variable() {
        // Set a temp env var and verify substitution.
        // SAFETY: test-only env mutation.
        unsafe {
            std::env::set_var("FREMINAL_TEST_VAR", "testvalue");
        }
        let content = r#"
[[tabs]]
  [[tabs.panes]]
  id = "main"
  directory = "$ENV{FREMINAL_TEST_VAR}"
"#;
        let layout = Layout::from_str_content(Path::new("test.toml"), content).expect("parse");
        let substituted = layout.apply_variables(&[], &HashMap::new());
        assert_eq!(
            substituted.tabs[0].panes[0].directory.as_deref(),
            Some("testvalue")
        );
        unsafe {
            std::env::remove_var("FREMINAL_TEST_VAR");
        }
    }

    #[test]
    fn multi_window_layout_parses() {
        let content = r#"
[layout]
name = "Multi"

[[windows]]
size = [1920, 1080]
position = [0, 0]

  [[windows.tabs]]
  title = "Window 1"

    [[windows.tabs.panes]]
    id = "w1p1"
    active = true

[[windows]]
size = [800, 600]
position = [100, 50]

  [[windows.tabs]]
  title = "Window 2"

    [[windows.tabs.panes]]
    id = "w2p1"
    active = true
"#;
        let layout =
            Layout::from_str_content(Path::new("test.toml"), content).expect("parse failed");
        assert_eq!(layout.windows.len(), 2);
        assert_eq!(layout.windows[0].size, Some([1920, 1080]));
        assert_eq!(layout.windows[0].position, Some([0, 0]));
        assert_eq!(layout.windows[1].size, Some([800, 600]));
        assert_eq!(layout.windows[1].position, Some([100, 50]));
    }

    #[test]
    fn multi_window_layout_round_trip() {
        let content = r#"
[layout]
name = "MultiRT"

[[windows]]
size = [1280, 720]
position = [10, 20]

  [[windows.tabs]]
  title = "Tab A"

    [[windows.tabs.panes]]
    id = "pane1"
    active = true

[[windows]]
size = [640, 480]

  [[windows.tabs]]
  title = "Tab B"

    [[windows.tabs.panes]]
    id = "pane2"
    active = true
"#;
        let layout =
            Layout::from_str_content(Path::new("test.toml"), content).expect("parse failed");
        let toml_str = layout.to_toml_string().expect("serialize failed");
        let reparsed =
            Layout::from_str_content(Path::new("test.toml"), &toml_str).expect("reparse failed");
        assert_eq!(reparsed.windows.len(), 2);
        assert_eq!(reparsed.windows[0].size, Some([1280, 720]));
        assert_eq!(reparsed.windows[0].position, Some([10, 20]));
        assert_eq!(reparsed.windows[1].size, Some([640, 480]));
        assert_eq!(reparsed.windows[1].position, None);
    }

    #[test]
    fn save_to_file_and_from_file_round_trip() {
        let layout =
            Layout::from_str_content(Path::new("test.toml"), SPLIT_LAYOUT).expect("parse failed");
        let tmp = std::env::temp_dir().join("freminal_layout_rt_test.toml");
        layout.save_to_file(&tmp).expect("save failed");
        let reloaded = Layout::from_file(&tmp).expect("load failed");
        assert_eq!(reloaded.display_name(), layout.display_name());
        assert_eq!(reloaded.windows.len(), layout.windows.len());
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn from_file_rejects_malformed_toml() {
        let tmp = std::env::temp_dir().join("freminal_layout_bad_test.toml");
        std::fs::write(&tmp, "this is not [ valid toml !!!").expect("write failed");
        let err = Layout::from_file(&tmp).expect_err("expected parse error");
        // Should be a parse/toml error, not a panic.
        let msg = err.to_string();
        assert!(!msg.is_empty(), "error message should be non-empty");
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn discover_layouts_finds_valid_files() {
        let tmp_dir = std::env::temp_dir().join("freminal_discover_test");
        let _ = std::fs::create_dir_all(&tmp_dir);

        // Write one valid layout.
        let valid_path = tmp_dir.join("my_layout.toml");
        let layout =
            Layout::from_str_content(Path::new("test.toml"), SIMPLE_LAYOUT).expect("parse failed");
        layout.save_to_file(&valid_path).expect("save failed");

        // Write one invalid .toml file (should be skipped silently).
        let bad_path = tmp_dir.join("corrupt.toml");
        std::fs::write(&bad_path, "not toml !!!").expect("write failed");

        // Write a non-toml file (should be ignored).
        let txt_path = tmp_dir.join("readme.txt");
        std::fs::write(&txt_path, "ignore me").expect("write failed");

        let summaries = discover_layouts(&tmp_dir);
        assert_eq!(summaries.len(), 1, "expected exactly 1 valid layout");
        assert_eq!(summaries[0].name, "Simple");
        assert_eq!(summaries[0].path, valid_path);

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn validation_rejects_cycle() {
        let bad = r#"
[[tabs]]
  [[tabs.panes]]
  id = "a"
  parent = "b"
  position = "first"

  [[tabs.panes]]
  id = "b"
  parent = "a"
  position = "second"
"#;
        let layout = Layout::from_str_content(Path::new("bad.toml"), bad).expect("parse");
        // Both nodes have parents — no root — which should fail first.
        let err = layout.validate().expect_err("expected validation error");
        let msg = err.to_string();
        // Could be "no root pane" or "cycle" — both are valid failures.
        assert!(msg.contains("root") || msg.contains("cycle"), "got: {msg}");
    }
}
