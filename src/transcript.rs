//! Read an agent's *transcript*, not its screen.
//!
//! `capture-pane` renders a TUI that redraws itself: ANSI noise, wrapped lines,
//! a scrollback that has already dropped the interesting part. Claude Code
//! meanwhile appends every turn as JSONL to `~/.claude/projects/<slug>/<id>.jsonl`.
//! That file is the ground truth, and it is structured — so an orchestrator can
//! ask for *just the final assistant message* and pay for nothing else.
//!
//! Token efficiency is the whole point of this module. Tool calls and their
//! results are the overwhelming bulk of a transcript and are almost never what
//! the caller wants, so they are dropped unless explicitly requested.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::GroveError;

/// Only ever read this much from the end of a transcript. Transcripts grow
/// without bound; the last few turns live in the last few hundred KiB.
const TAIL_BYTES: u64 = 4 * 1024 * 1024;

/// What to pull out of a transcript.
#[derive(Debug, Clone)]
pub struct ReadOpts {
    /// How many trailing assistant turns to include.
    pub turns: usize,
    /// Annotate each turn with the tools it called.
    pub tools: bool,
    /// Include user/tool-result turns too. Expensive — opt in.
    pub full: bool,
    /// Hard ceiling on returned characters. 0 disables the cap.
    pub max_chars: usize,
}

impl Default for ReadOpts {
    fn default() -> Self {
        Self {
            turns: 1,
            tools: false,
            full: false,
            max_chars: 4000,
        }
    }
}

/// An excerpt of a transcript, ready to hand to a caller.
#[derive(Debug, Clone)]
pub struct Excerpt {
    pub text: String,
    pub truncated: bool,
    pub source: PathBuf,
}

/// One turn, flattened from its content blocks.
#[derive(Debug, Clone, PartialEq)]
struct Turn {
    role: String,
    text: String,
    tools: Vec<String>,
}

#[derive(Deserialize)]
struct Entry {
    #[serde(default)]
    message: Option<Message>,
    /// Sub-agent (Task tool) turns are recorded in the same file. They are not
    /// the agent's own answer, so they never count as "the last message".
    #[serde(default, rename = "isSidechain")]
    is_sidechain: bool,
}

#[derive(Deserialize)]
struct Message {
    #[serde(default)]
    role: String,
    #[serde(default)]
    content: serde_json::Value,
}

/// Flatten a message's content blocks. `content` is either a bare string or an
/// array of typed blocks; both shapes appear in real transcripts.
fn flatten(content: &serde_json::Value) -> (String, Vec<String>) {
    if let Some(s) = content.as_str() {
        return (s.to_string(), Vec::new());
    }
    let Some(blocks) = content.as_array() else {
        return (String::new(), Vec::new());
    };

    let mut text = String::new();
    let mut tools = Vec::new();
    for block in blocks {
        match block.get("type").and_then(serde_json::Value::as_str) {
            Some("text") => {
                if let Some(t) = block.get("text").and_then(serde_json::Value::as_str) {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(t);
                }
            }
            Some("tool_use") => {
                if let Some(name) = block.get("name").and_then(serde_json::Value::as_str) {
                    tools.push(name.to_string());
                }
            }
            Some("tool_result") => {
                // Only ever surfaced by --full; the default read drops it.
                if let Some(c) = block.get("content") {
                    let body = c
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| c.to_string());
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&body);
                }
            }
            _ => {}
        }
    }
    (text, tools)
}

/// Parse JSONL into turns, skipping sidechains and unparseable lines. A
/// truncated first line (from a tail read) simply fails to parse and is dropped.
///
/// One assistant turn spans *several* JSONL messages: Claude emits its prose and
/// each tool call as separate entries. So consecutive assistant messages are
/// coalesced into one turn — otherwise "the last assistant message" is usually a
/// bare `tool_use` with no text, and a caller asking what the agent said gets
/// nothing back.
fn parse_turns(raw: &str) -> Vec<Turn> {
    let messages = raw
        .lines()
        .filter_map(|line| serde_json::from_str::<Entry>(line).ok())
        .filter(|e| !e.is_sidechain)
        .filter_map(|e| e.message);

    let mut turns: Vec<Turn> = Vec::new();
    for m in messages {
        let (text, tools) = flatten(&m.content);
        if text.is_empty() && tools.is_empty() {
            continue;
        }
        match turns.last_mut() {
            Some(last) if last.role == m.role && m.role == "assistant" => {
                if !text.is_empty() {
                    if !last.text.is_empty() {
                        last.text.push('\n');
                    }
                    last.text.push_str(&text);
                }
                last.tools.extend(tools);
            }
            _ => turns.push(Turn {
                role: m.role,
                text,
                tools,
            }),
        }
    }
    turns
}

/// Read at most the last `TAIL_BYTES` of a file. Reading a multi-MiB transcript
/// in full to quote its last paragraph is exactly the waste this module exists
/// to avoid.
fn read_tail(path: &Path, whole: bool) -> Result<String, GroveError> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path).map_err(|e| {
        GroveError::General(format!("cannot read transcript {}: {e}", path.display()))
    })?;
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);

    if !whole && len > TAIL_BYTES {
        file.seek(SeekFrom::Start(len - TAIL_BYTES))?;
    }
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Keep the head and the tail of an over-long excerpt: the opening states what
/// the agent did, the ending states where it landed. The middle is the padding.
fn cap(text: &str, max_chars: usize) -> (String, bool) {
    if max_chars == 0 || text.chars().count() <= max_chars {
        return (text.to_string(), false);
    }
    let chars: Vec<char> = text.chars().collect();
    let half = max_chars / 2;
    let head: String = chars[..half].iter().collect();
    let tail: String = chars[chars.len() - half..].iter().collect();
    let elided = chars.len() - 2 * half;
    (
        format!("{head}\n\n[... {elided} chars elided — rerun with --max-chars 0 for all ...]\n\n{tail}"),
        true,
    )
}

/// Render the requested slice of a transcript.
pub fn read_excerpt(path: &Path, opts: &ReadOpts) -> Result<Excerpt, GroveError> {
    let raw = read_tail(path, opts.full)?;
    let turns = parse_turns(&raw);

    // A turn that only called tools is not something the agent *said*. Skipping
    // it is what makes `grove read` answer "what happened" rather than "what was
    // the last thing written to the file".
    let wanted: Vec<&Turn> = if opts.full {
        turns.iter().collect()
    } else {
        let spoken: Vec<&Turn> = turns
            .iter()
            .filter(|t| t.role == "assistant" && !t.text.is_empty())
            .collect();
        let skip = spoken.len().saturating_sub(opts.turns.max(1));
        spoken.into_iter().skip(skip).collect()
    };

    if wanted.is_empty() {
        return Err(GroveError::General(format!(
            "no agent turns found in {}",
            path.display()
        )));
    }

    let show_tools = opts.tools || opts.full;
    let rendered = wanted
        .iter()
        .map(|t| {
            let mut out = String::new();
            if opts.full {
                out.push_str(&format!("--- {} ---\n", t.role));
            }
            if show_tools && !t.tools.is_empty() {
                out.push_str(&format!("[tools: {}]\n", t.tools.join(", ")));
            }
            out.push_str(&t.text);
            out
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    let (text, truncated) = cap(rendered.trim(), opts.max_chars);
    Ok(Excerpt {
        text,
        truncated,
        source: path.to_path_buf(),
    })
}

/// Claude's project-directory slug: every character that is not alphanumeric or
/// `-` becomes `-`, so `/Users/a.b/.claude` → `-Users-a-b--claude`.
pub fn project_slug(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Locate a transcript for a working directory by convention.
///
/// Fallback only — the hook records the real path, which is authoritative. This
/// exists so `grove read` still works for an agent that was launched before the
/// hook was wired up.
pub fn discover(cwd: &Path) -> Option<PathBuf> {
    let dir = dirs::home_dir()?
        .join(".claude")
        .join("projects")
        .join(project_slug(cwd));

    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .max_by_key(|(mtime, _)| *mtime)
        .map(|(_, path)| path)
}

/// Resolve which transcript to read: the hook-recorded path if we have one and
/// it still exists, else convention.
pub fn resolve(recorded: Option<&Path>, cwd: &Path) -> Option<PathBuf> {
    recorded
        .filter(|p| p.exists())
        .map(Path::to_path_buf)
        .or_else(|| discover(cwd))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jsonl(lines: &[&str]) -> String {
        lines.join("\n")
    }

    const TOOL_TURN: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}"#;
    const TOOL_RESULT: &str = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"a.rs\nb.rs"}]}}"#;
    const FINAL_TURN: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Done. Fixed the parser."}]}}"#;

    fn write(dir: &tempfile::TempDir, body: &str) -> PathBuf {
        let path = dir.path().join("session.jsonl");
        std::fs::write(&path, body).unwrap();
        path
    }

    /// The default read is the whole point: the last thing the agent *said*,
    /// with none of the tool traffic that dwarfs it.
    #[test]
    fn default_read_returns_only_the_final_assistant_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, &jsonl(&[TOOL_TURN, TOOL_RESULT, FINAL_TURN]));

        let out = read_excerpt(&path, &ReadOpts::default()).unwrap();

        assert_eq!(out.text, "Done. Fixed the parser.");
        assert!(!out.truncated);
    }

    /// A sub-agent's turn is not the agent's answer. Returning it would hand the
    /// orchestrator a sub-agent's words as if the task had reported them.
    #[test]
    fn sidechain_turns_are_never_the_last_message() {
        let sidechain = r#"{"type":"assistant","isSidechain":true,"message":{"role":"assistant","content":[{"type":"text","text":"subagent chatter"}]}}"#;
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, &jsonl(&[FINAL_TURN, sidechain]));

        let out = read_excerpt(&path, &ReadOpts::default()).unwrap();

        assert_eq!(out.text, "Done. Fixed the parser.");
    }

    /// Regression: real transcripts split one assistant turn across several
    /// messages and frequently *end* on a bare `tool_use`. Taking the literal
    /// last assistant message returned an empty string — the agent's actual words
    /// were one message earlier.
    #[test]
    fn trailing_tool_call_does_not_swallow_the_agents_words() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, &jsonl(&[FINAL_TURN, TOOL_TURN, TOOL_RESULT]));

        let out = read_excerpt(&path, &ReadOpts::default()).unwrap();

        assert_eq!(out.text, "Done. Fixed the parser.");
    }

    /// The flip side: prose and tool calls emitted as separate messages are one
    /// turn, so a single-turn read must return all of the prose, not the last
    /// fragment of it.
    #[test]
    fn consecutive_assistant_messages_coalesce_into_one_turn() {
        let first = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Checking the parser."}]}}"#;
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, &jsonl(&[first, TOOL_TURN, FINAL_TURN]));

        let out = read_excerpt(&path, &ReadOpts::default()).unwrap();

        assert_eq!(out.text, "Checking the parser.\nDone. Fixed the parser.");
    }

    #[test]
    fn tools_are_listed_only_when_asked_for() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, &jsonl(&[TOOL_TURN, FINAL_TURN]));

        let opts = ReadOpts {
            turns: 2,
            tools: true,
            ..Default::default()
        };
        let out = read_excerpt(&path, &opts).unwrap();

        assert!(out.text.contains("[tools: Bash]"), "got: {}", out.text);
    }

    /// A runaway agent must not be able to blow up the orchestrator's context.
    #[test]
    fn oversized_output_is_capped_and_flagged() {
        let long = "x".repeat(5000);
        let line = format!(
            r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"{long}"}}]}}}}"#
        );
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, &line);

        let opts = ReadOpts {
            max_chars: 100,
            ..Default::default()
        };
        let out = read_excerpt(&path, &opts).unwrap();

        assert!(out.truncated);
        assert!(out.text.chars().count() < 500);
        assert!(out.text.contains("elided"));
    }

    #[test]
    fn slug_matches_claude_project_dir_naming() {
        assert_eq!(
            project_slug(Path::new("/Users/a.b/.claude")),
            "-Users-a-b--claude"
        );
    }

    /// A garbled first line is what a tail read produces; it must not poison the
    /// parse of every well-formed line after it.
    #[test]
    fn partial_leading_line_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, &jsonl(&["ontent\":[{\"type\":\"te", FINAL_TURN]));

        let out = read_excerpt(&path, &ReadOpts::default()).unwrap();

        assert_eq!(out.text, "Done. Fixed the parser.");
    }
}
