// 解説結果のMarkdown整形プレビュー (SPECv0.5.2追補)
// pulldown-cmark でパースし、GDIで自前描画できるブロック単位の中間表現へ変換する。
// GDIのDrawTextWは1回の呼び出しにつき単一フォント/色しか扱えないため、太字等の
// インライン強調は「ブロック全体が強調で覆われている場合のみ」bold=trueとして表現する
// (文中の部分強調はマーカーを外した地の文として表示する簡易実装)。
// コピーチップは常にMarkdown原文(パース前の文字列)を扱うため、この変換結果を使わない。
use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};

/// ブロックの種別
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlockKind {
    Heading(u8),
    Paragraph,
    /// 箇条書き (番号付き/番号なし共通。インデント深さのみ保持)
    ListItem { depth: u8 },
    CodeBlock,
    Rule,
}

/// 描画用の1ブロック (見出し/段落/リスト項目/コードブロック/水平線)
#[derive(Clone, Debug)]
pub struct Block {
    pub kind: BlockKind,
    pub text: String,
    /// ブロック全体が太字強調で覆われているか
    pub bold: bool,
}

/// Markdown文字列をブロック列へ変換する。空文字列や解析結果が空なら空配列。
pub fn parse(md: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut cur = String::new();
    let mut cur_kind = BlockKind::Paragraph;
    let mut list_depth: u8 = 0;
    let mut ordered_index: Vec<Option<u64>> = Vec::new();
    let mut strong_depth = 0u32;
    let mut all_bold = true;
    let mut has_text = false;

    let flush = |blocks: &mut Vec<Block>, cur: &mut String, kind: BlockKind, all_bold: &mut bool, has_text: &mut bool| {
        let text = cur.trim_end().to_string();
        if !text.is_empty() {
            blocks.push(Block { kind, text, bold: *all_bold && *has_text });
        }
        cur.clear();
        *all_bold = true;
        *has_text = false;
    };

    for ev in Parser::new(md) {
        match ev {
            Event::Start(Tag::Heading { level, .. }) => {
                flush(&mut blocks, &mut cur, cur_kind, &mut all_bold, &mut has_text);
                cur_kind = BlockKind::Heading(heading_level_num(level));
            }
            Event::End(TagEnd::Heading(_)) => {
                flush(&mut blocks, &mut cur, cur_kind, &mut all_bold, &mut has_text);
                cur_kind = BlockKind::Paragraph;
            }
            Event::Start(Tag::Paragraph) => {
                flush(&mut blocks, &mut cur, cur_kind, &mut all_bold, &mut has_text);
                cur_kind = if list_depth > 0 {
                    BlockKind::ListItem { depth: list_depth - 1 }
                } else {
                    BlockKind::Paragraph
                };
            }
            Event::End(TagEnd::Paragraph) => {
                flush(&mut blocks, &mut cur, cur_kind, &mut all_bold, &mut has_text);
                cur_kind = BlockKind::Paragraph;
            }
            Event::Start(Tag::List(start)) => {
                list_depth += 1;
                ordered_index.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                list_depth = list_depth.saturating_sub(1);
                ordered_index.pop();
            }
            Event::Start(Tag::Item) => {
                flush(&mut blocks, &mut cur, cur_kind, &mut all_bold, &mut has_text);
                let depth = list_depth.saturating_sub(1);
                cur_kind = BlockKind::ListItem { depth };
                if let Some(Some(n)) = ordered_index.last_mut() {
                    cur.push_str(&format!("{n}. "));
                    *n += 1;
                } else {
                    cur.push_str("• ");
                }
            }
            Event::End(TagEnd::Item) => {
                flush(&mut blocks, &mut cur, cur_kind, &mut all_bold, &mut has_text);
                cur_kind = BlockKind::Paragraph;
            }
            Event::Start(Tag::CodeBlock(_)) => {
                flush(&mut blocks, &mut cur, cur_kind, &mut all_bold, &mut has_text);
                cur_kind = BlockKind::CodeBlock;
            }
            Event::End(TagEnd::CodeBlock) => {
                flush(&mut blocks, &mut cur, cur_kind, &mut all_bold, &mut has_text);
                cur_kind = BlockKind::Paragraph;
            }
            Event::Start(Tag::Strong) => strong_depth += 1,
            Event::End(TagEnd::Strong) => strong_depth = strong_depth.saturating_sub(1),
            Event::Rule => {
                flush(&mut blocks, &mut cur, cur_kind, &mut all_bold, &mut has_text);
                blocks.push(Block { kind: BlockKind::Rule, text: String::new(), bold: false });
            }
            Event::Text(t) | Event::Code(t) => {
                cur.push_str(&t);
                has_text = true;
                if strong_depth == 0 {
                    all_bold = false;
                }
            }
            Event::SoftBreak | Event::HardBreak => cur.push('\n'),
            _ => {}
        }
    }
    flush(&mut blocks, &mut cur, cur_kind, &mut all_bold, &mut has_text);
    blocks
}

fn heading_level_num(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_heading_paragraph_and_bold_paragraph() {
        let blocks = parse("# 見出し\n\n本文です。\n\n**全部太字**");
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].kind, BlockKind::Heading(1));
        assert_eq!(blocks[0].text, "見出し");
        assert_eq!(blocks[1].kind, BlockKind::Paragraph);
        assert!(!blocks[1].bold);
        assert_eq!(blocks[2].text, "全部太字");
        assert!(blocks[2].bold, "段落全体が強調のときはboldになる");
    }

    #[test]
    fn mixed_bold_paragraph_is_not_fully_bold() {
        let blocks = parse("普通の文と**一部太字**が混在。");
        assert_eq!(blocks.len(), 1);
        assert!(!blocks[0].bold, "一部だけ強調の段落は全体boldにしない");
        assert_eq!(blocks[0].text, "普通の文と一部太字が混在。");
    }

    #[test]
    fn parses_unordered_list_items_with_prefix() {
        let blocks = parse("- 項目1\n- 項目2\n");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].kind, BlockKind::ListItem { depth: 0 });
        assert_eq!(blocks[0].text, "• 項目1");
        assert_eq!(blocks[1].text, "• 項目2");
    }

    #[test]
    fn parses_ordered_list_items_with_numbers() {
        let blocks = parse("1. 一番目\n2. 二番目\n");
        assert_eq!(blocks[0].text, "1. 一番目");
        assert_eq!(blocks[1].text, "2. 二番目");
    }

    #[test]
    fn parses_code_block_preserving_lines() {
        let blocks = parse("```\nfn main() {}\nlet x = 1;\n```");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, BlockKind::CodeBlock);
        assert!(blocks[0].text.contains("fn main() {}"));
        assert!(blocks[0].text.contains("let x = 1;"));
    }

    #[test]
    fn parses_horizontal_rule() {
        let blocks = parse("前\n\n---\n\n後");
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[1].kind, BlockKind::Rule);
    }

    #[test]
    fn empty_input_yields_no_blocks() {
        assert!(parse("").is_empty());
        assert!(parse("   \n  ").is_empty());
    }
}
