//! Splits a Markdown file into its top-level blocks for the live preview
//! (`prototype/markdown_preview.rs`): every block renders on its own and turns into its source
//! when clicked. The parser is the one Kit's `TextView` already uses.

use ::markdown::{ParseOptions, mdast::Node};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Markdown,
    /// YAML / TOML front matter, shown as a code block in that language.
    Frontmatter(&'static str),
    /// A link reference definition renders as nothing, so its source is shown instead.
    Definition,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    /// Byte range in the file.
    pub range: Range<usize>,
    pub kind: Kind,
}

#[derive(Debug, Default, PartialEq)]
pub struct Split {
    pub blocks: Vec<Block>,
    /// The source of every link reference definition, one per line. A block that renders alone
    /// needs them appended to resolve `[text][id]`.
    pub definitions: String,
}

fn options() -> ParseOptions {
    let mut options = ParseOptions::gfm();
    options.constructs.frontmatter = true;
    // As Kit parses: a `$$` fence holding blank lines is one block.
    options.constructs.math_text = true;
    options.constructs.math_flow = true;
    options
}

pub fn split(text: &str) -> Split {
    let children = match ::markdown::to_mdast(text, &options()) {
        Ok(Node::Root(root)) => root.children,
        // Only MDX syntax errors fail; plain Markdown always parses.
        _ => {
            return Split {
                blocks: (!text.trim().is_empty())
                    .then_some(Block {
                        range: 0..text.len(),
                        kind: Kind::Markdown,
                    })
                    .into_iter()
                    .collect(),
                definitions: String::new(),
            };
        }
    };
    let mut split = Split::default();
    for node in &children {
        let Some(position) = node.position() else {
            continue;
        };
        // A loose list ends after its last blank line.
        let start = position.start.offset;
        let range = start..start + text[start..position.end.offset].trim_end().len();
        let kind = match node {
            Node::Yaml(_) => Kind::Frontmatter("yaml"),
            Node::Toml(_) => Kind::Frontmatter("toml"),
            Node::Definition(_) => {
                split.definitions.push_str(&text[range.clone()]);
                split.definitions.push('\n');
                Kind::Definition
            }
            _ => Kind::Markdown,
        };
        split.blocks.push(Block { range, kind });
    }
    split
}

/// What a block renders: its source, with the definitions appended when it may use one, or a
/// fenced code block for front matter.
pub fn rendered(text: &str, block: &Block, definitions: &str) -> String {
    let source = &text[block.range.clone()];
    match block.kind {
        Kind::Frontmatter(language) => format!("```{language}\n{source}\n```"),
        Kind::Markdown if !definitions.is_empty() && source.contains(']') => {
            format!("{source}\n\n{definitions}")
        }
        Kind::Markdown | Kind::Definition => source.to_string(),
    }
}

/// The separator to put before text typed into a new block at the end of `text`, so it does
/// not join the last block.
pub fn separator_at_end(text: &str) -> &'static str {
    if text.is_empty() || text.ends_with("\n\n") {
        ""
    } else if text.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources(text: &str) -> Vec<&str> {
        split(text)
            .blocks
            .iter()
            .map(|block| &text[block.range.clone()])
            .collect()
    }

    #[test]
    fn top_level_blocks_keep_their_blank_lines_inside() {
        let text = "# 标题\n\n第一段，**粗体**\n第二行\n\n- 一\n\n- 二\n\n```rust\nfn a() {}\n\nfn b() {}\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        assert_eq!(
            sources(text),
            [
                "# 标题",
                "第一段，**粗体**\n第二行",
                "- 一\n\n- 二",
                "```rust\nfn a() {}\n\nfn b() {}\n```",
                "| a | b |\n|---|---|\n| 1 | 2 |",
            ]
        );
    }

    #[test]
    fn front_matter_and_definitions_are_their_own_kinds() {
        let text = "---\ntitle: 说明\n---\n\n见 [文档][doc]。\n\n[doc]: https://example.com\n";
        let split = split(text);
        let kinds: Vec<Kind> = split.blocks.iter().map(|block| block.kind).collect();
        assert_eq!(
            kinds,
            [Kind::Frontmatter("yaml"), Kind::Markdown, Kind::Definition]
        );
        assert_eq!(split.definitions, "[doc]: https://example.com\n");
        assert_eq!(
            rendered(text, &split.blocks[0], &split.definitions),
            "```yaml\n---\ntitle: 说明\n---\n```"
        );
        assert_eq!(
            rendered(text, &split.blocks[1], &split.definitions),
            "见 [文档][doc]。\n\n[doc]: https://example.com\n"
        );
    }

    #[test]
    fn empty_text_has_no_blocks_and_math_stays_whole() {
        assert!(split("").blocks.is_empty());
        assert!(split("\n\n  \n").blocks.is_empty());
        assert_eq!(sources("$$\na\n\nb\n$$\n"), ["$$\na\n\nb\n$$"]);
    }

    #[test]
    fn new_blocks_start_after_a_blank_line() {
        assert_eq!(separator_at_end(""), "");
        assert_eq!(separator_at_end("a"), "\n\n");
        assert_eq!(separator_at_end("a\n"), "\n");
        assert_eq!(separator_at_end("a\n\n"), "");
    }
}
