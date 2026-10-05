//! A deliberately small Markdown subset for agent replies: paragraphs, headings, bullet and
//! numbered lists, block quotes, rules, fenced code (highlighted with the editor's syntax
//! theme), inline code, bold, italic and links. Parsing is a linear scan; highlighting runs in
//! the background once a reply is complete.

use gpui_kit::{HighlightStyle, component::highlighter::HighlightTheme};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Span {
    Code,
    Bold,
    Italic,
    Link,
}

/// Text with styled byte ranges.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inline {
    pub text: String,
    pub spans: Vec<(Range<usize>, Span)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Paragraph(Inline),
    Heading(Inline),
    /// `marker` is "•" or "3."; `depth` counts nesting from 0.
    Item {
        depth: usize,
        marker: String,
        text: Inline,
    },
    Quote(Inline),
    Rule,
    Code {
        language: Option<&'static str>,
        text: String,
        /// Syntax runs over `text`; empty until highlighted.
        runs: Vec<(Range<usize>, HighlightStyle)>,
    },
}

/// Fence tag → the highlighter's language name (only grammars compiled into Kit).
fn fence_language(tag: &str) -> Option<&'static str> {
    let tag = tag.trim().to_ascii_lowercase();
    let ext = match tag.as_str() {
        "" => return None,
        "rust" | "rs" => "rs",
        "python" | "py" => "py",
        "javascript" | "js" | "jsx" => "js",
        "typescript" | "ts" => "ts",
        "tsx" => "tsx",
        "bash" | "sh" | "shell" | "zsh" | "console" => "sh",
        "go" | "golang" => "go",
        "c" | "h" => "c",
        "cpp" | "c++" | "cc" | "hpp" => "cpp",
        "java" => "java",
        "sql" => "sql",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "json" | "jsonc" => "json",
        "html" => "html",
        "css" => "css",
        "markdown" | "md" => "md",
        "diff" | "patch" => "diff",
        _ => return None,
    };
    match crate::languages::for_path(std::path::Path::new(&format!("x.{ext}"))).0 {
        "plain" => None,
        language => Some(language),
    }
}

fn fence(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    trimmed
        .strip_prefix("```")
        .or_else(|| trimmed.strip_prefix("~~~"))
}

/// Inline markup: `code`, **bold**, *italic*, [text](url). Unclosed markers stay literal.
pub fn inline(source: &str) -> Inline {
    let mut out = Inline::default();
    let bytes = source.as_bytes();
    let mut i = 0;
    let mut plain_start = 0;
    let flush = |out: &mut Inline, from: usize, to: usize| {
        out.text.push_str(&source[from..to]);
    };
    while i < bytes.len() {
        let rest = &source[i..];
        let styled: Option<(usize, &str, Span)> = if let Some(after) = rest.strip_prefix('`') {
            after
                .find('`')
                .map(|end| (end + 2, &after[..end], Span::Code))
        } else if let Some(after) = rest.strip_prefix("**") {
            after
                .find("**")
                .filter(|&end| end > 0)
                .map(|end| (end + 4, &after[..end], Span::Bold))
        } else if let Some(after) = rest.strip_prefix('*')
            && after.chars().next().is_some_and(|c| !c.is_whitespace())
            // `2*3*4` and `a*b*c` stay literal (agents write arithmetic and pointers).
            && !source[..i].chars().next_back().is_some_and(char::is_alphanumeric)
        {
            after
                .find('*')
                .filter(|&end| end > 0)
                .map(|end| (end + 2, &after[..end], Span::Italic))
        } else if rest.starts_with('[') {
            rest.find("](").and_then(|mid| {
                rest[mid + 2..]
                    .find(')')
                    .map(|end| (mid + 2 + end + 1, &rest[1..mid], Span::Link))
            })
        } else {
            None
        };
        match styled {
            Some((consumed, inner, span)) if !inner.contains('\n') || span == Span::Code => {
                flush(&mut out, plain_start, i);
                let start = out.text.len();
                out.text.push_str(inner);
                out.spans.push((start..out.text.len(), span));
                i += consumed;
                plain_start = i;
            }
            _ => {
                i += rest.chars().next().map_or(1, char::len_utf8);
            }
        }
    }
    flush(&mut out, plain_start, bytes.len());
    out
}

fn list_item(line: &str) -> Option<(usize, String, &str)> {
    let indent = line.len() - line.trim_start().len();
    let trimmed = line.trim_start();
    let depth = indent / 2;
    for bullet in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(bullet) {
            let rest = rest
                .strip_prefix("[ ] ")
                .map(|r| ("☐", r))
                .or_else(|| rest.strip_prefix("[x] ").map(|r| ("☑", r)));
            return Some(match rest {
                Some((mark, r)) => (depth, mark.to_string(), r),
                None => (depth, "•".to_string(), &trimmed[2..]),
            });
        }
    }
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 && digits < 4 {
        let after = &trimmed[digits..];
        if let Some(rest) = after
            .strip_prefix(". ")
            .or_else(|| after.strip_prefix(") "))
        {
            return Some((depth, format!("{}.", &trimmed[..digits]), rest));
        }
    }
    None
}

/// Parses the whole reply. An unclosed fence (still streaming) becomes a code block too.
pub fn parse(source: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let mut lines = source.lines().peekable();
    let flush = |paragraph: &mut Vec<&str>, blocks: &mut Vec<Block>| {
        if !paragraph.is_empty() {
            blocks.push(Block::Paragraph(inline(&paragraph.join("\n"))));
            paragraph.clear();
        }
    };
    while let Some(line) = lines.next() {
        if let Some(tag) = fence(line) {
            flush(&mut paragraph, &mut blocks);
            let mut code = String::new();
            for body in lines.by_ref() {
                if fence(body).is_some_and(|rest| rest.trim().is_empty()) {
                    break;
                }
                code.push_str(body);
                code.push('\n');
            }
            if code.ends_with('\n') {
                code.pop();
            }
            blocks.push(Block::Code {
                language: fence_language(tag),
                text: code,
                runs: Vec::new(),
            });
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            flush(&mut paragraph, &mut blocks);
            continue;
        }
        if matches!(trimmed, "---" | "***" | "___") {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Rule);
            continue;
        }
        let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Heading(inline(trimmed[hashes..].trim())));
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('>') {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Quote(inline(rest.trim_start())));
            continue;
        }
        if let Some((depth, marker, rest)) = list_item(line) {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Item {
                depth,
                marker,
                text: inline(rest),
            });
            continue;
        }
        paragraph.push(line.trim_end());
    }
    flush(&mut paragraph, &mut blocks);
    blocks
}

/// Parses a reply while it streams without starting over each time: everything before the
/// last blank line outside a code fence is final (the parser is line based and only a
/// paragraph or an open fence carries across lines), so only the tail is parsed again.
#[derive(Default)]
pub struct Streaming {
    /// Bytes of the source whose blocks are final.
    stable_len: usize,
    stable: Vec<Block>,
}

impl Streaming {
    /// The blocks of `source`, which must extend the source of the previous call.
    pub fn update(&mut self, source: &str) -> Vec<Block> {
        if source.len() < self.stable_len || !source.is_char_boundary(self.stable_len) {
            *self = Self::default();
        }
        let split = stable_end(source, self.stable_len);
        if split > self.stable_len {
            self.stable.extend(parse(&source[self.stable_len..split]));
            self.stable_len = split;
        }
        let mut blocks = self.stable.clone();
        blocks.extend(parse(&source[self.stable_len..]));
        blocks
    }
}

/// The end of the last blank line after `from` (a line start outside a fence) that is not
/// inside a code fence: parsing stops and restarts there without changing the result.
fn stable_end(source: &str, from: usize) -> usize {
    let mut end = from;
    let mut in_fence = false;
    let mut at = from;
    for line in source[from..].split_inclusive('\n') {
        at += line.len();
        if !line.ends_with('\n') {
            break; // still being written
        }
        let text = line.trim_end_matches(['\n', '\r']);
        if in_fence {
            if fence(text).is_some_and(|rest| rest.trim().is_empty()) {
                in_fence = false;
            }
        } else if fence(text).is_some() {
            in_fence = true;
        } else if text.trim().is_empty() {
            end = at;
        }
    }
    end
}

/// Fills in syntax runs for fenced code (off the UI thread).
pub fn highlight(blocks: &mut [Block], theme: &HighlightTheme) {
    for block in blocks {
        if let Block::Code {
            language: Some(language),
            text,
            runs,
        } = block
            && text.len() <= 256 * 1024
        {
            *runs = crate::diff_doc::highlight(language, text, theme);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_matches_a_full_parse_at_every_step() {
        let reply = "# 标题\n\n第一段\n续行 **粗体**\n\n- a\n- b\n\n```rust\nfn main() {\n\n}\n```\n\n> 引用\n\n---\n\n最后一段 `code`\n";
        let mut streaming = Streaming::default();
        let mut end = 0;
        while end < reply.len() {
            end += 1;
            while !reply.is_char_boundary(end) {
                end += 1;
            }
            assert_eq!(
                streaming.update(&reply[..end]),
                parse(&reply[..end]),
                "{end}"
            );
        }
        assert!(streaming.stable_len > 0);
        // A source that does not extend the previous one starts over.
        assert_eq!(streaming.update("新的"), parse("新的"));
    }

    #[test]
    fn blocks_and_inline_markup() {
        let blocks = parse(
            "## 原因\n原因在 `on_reconnect`：它只**重置**了退避状态，\n见 [文档](https://x)。\n\n- 第一点\n  - 嵌套\n3. 第三\n- [x] 已完成\n\n```rust\nfn main() {}\n```\n> 注意\n---\n",
        );
        assert!(matches!(&blocks[0], Block::Heading(t) if t.text == "原因"));
        match &blocks[1] {
            Block::Paragraph(p) => {
                assert_eq!(
                    p.text,
                    "原因在 on_reconnect：它只重置了退避状态，\n见 文档。"
                );
                let spans: Vec<(&str, Span)> = p
                    .spans
                    .iter()
                    .map(|(r, s)| (&p.text[r.clone()], *s))
                    .collect();
                assert_eq!(
                    spans,
                    vec![
                        ("on_reconnect", Span::Code),
                        ("重置", Span::Bold),
                        ("文档", Span::Link)
                    ]
                );
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(&blocks[2], Block::Item { depth: 0, marker, .. } if marker == "•"));
        assert!(matches!(&blocks[3], Block::Item { depth: 1, .. }));
        assert!(matches!(&blocks[4], Block::Item { marker, .. } if marker == "3."));
        assert!(matches!(&blocks[5], Block::Item { marker, .. } if marker == "☑"));
        assert!(matches!(
            &blocks[6],
            Block::Code { language: Some("rust"), text, .. } if text == "fn main() {}"
        ));
        assert!(matches!(&blocks[7], Block::Quote(q) if q.text == "注意"));
        assert_eq!(blocks[8], Block::Rule);
    }

    #[test]
    fn literal_markers_and_streaming_fences() {
        let p = inline("a * b, snake_case, `unclosed, 2*3*4");
        assert_eq!(p.text, "a * b, snake_case, `unclosed, 2*3*4");
        assert!(p.spans.is_empty(), "{p:?}");
        assert_eq!(inline("a *b* c").spans, vec![(2..3, Span::Italic)]);
        let blocks = parse("看这里：\n```py\nprint(1)\n");
        assert!(matches!(
            &blocks[1],
            Block::Code { language: Some("python"), text, .. } if text == "print(1)"
        ));
    }

    #[test]
    fn code_blocks_get_syntax_runs() {
        let mut blocks = parse("```rust\nfn main() { let x = 1; }\n```");
        highlight(&mut blocks, &crate::theme::highlight_theme_for_tests(true));
        match &blocks[0] {
            Block::Code { runs, .. } => assert!(!runs.is_empty()),
            other => panic!("{other:?}"),
        }
    }
}
