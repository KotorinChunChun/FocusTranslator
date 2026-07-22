// 解説結果のMarkdown整形プレビュー (SPECv0.5.2追補)
// pulldown-cmark でパースし、GDIで自前描画できるブロック単位の中間表現へ変換する。
// GDIのDrawTextWは1回の呼び出しにつき単一フォント/色しか扱えないため、太字等の
// インライン強調は「ブロック全体が強調で覆われている場合のみ」bold=trueとして表現する
// (文中の部分強調はマーカーを外した地の文として表示する簡易実装)。
// コピーチップは常にMarkdown原文(パース前の文字列)を扱うため、この変換結果を使わない。
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// ブロックの種別
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlockKind {
    Heading(u8),
    Paragraph,
    /// 箇条書き (番号付き/番号なし共通。インデント深さのみ保持)
    ListItem { depth: u8 },
    CodeBlock,
    Rule,
    /// 表 (GFMテーブル)。text には等幅フォント向けに桁揃えした複数行文字列が入る (SPECv0.5.4 §5)
    Table,
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

    // 表 (GFMテーブル) の収集状態。in_table 中は Text をセルへ溜める (SPECv0.5.4 §5)。
    let mut in_table = false;
    let mut table_rows: Vec<Vec<String>> = Vec::new();
    let mut table_cur_row: Vec<String> = Vec::new();
    let mut table_cur_cell = String::new();

    let flush = |blocks: &mut Vec<Block>, cur: &mut String, kind: BlockKind, all_bold: &mut bool, has_text: &mut bool| {
        let text = cur.trim_end().to_string();
        if !text.is_empty() {
            blocks.push(Block { kind, text, bold: *all_bold && *has_text });
        }
        cur.clear();
        *all_bold = true;
        *has_text = false;
    };

    // テーブル記法 (`| A | B |`) を解析対象にする (SPECv0.5.4 §5)
    for ev in Parser::new_ext(md, Options::ENABLE_TABLES) {
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
            Event::Start(Tag::Table(_)) => {
                flush(&mut blocks, &mut cur, cur_kind, &mut all_bold, &mut has_text);
                in_table = true;
                table_rows.clear();
                table_cur_row.clear();
                table_cur_cell.clear();
            }
            Event::End(TagEnd::Table) => {
                let text = format_table(&table_rows);
                if !text.is_empty() {
                    blocks.push(Block { kind: BlockKind::Table, text, bold: false });
                }
                in_table = false;
                table_rows.clear();
                cur_kind = BlockKind::Paragraph;
            }
            // TableHead(ヘッダー行) と TableRow(本文行) はどちらも1行として扱う。
            Event::Start(Tag::TableHead) | Event::Start(Tag::TableRow) => table_cur_row.clear(),
            Event::End(TagEnd::TableHead) | Event::End(TagEnd::TableRow) => {
                table_rows.push(std::mem::take(&mut table_cur_row));
            }
            Event::Start(Tag::TableCell) => table_cur_cell.clear(),
            Event::End(TagEnd::TableCell) => {
                table_cur_row.push(table_cur_cell.trim().to_string());
                table_cur_cell.clear();
            }
            Event::Text(t) | Event::Code(t) => {
                if in_table {
                    table_cur_cell.push_str(&t);
                } else {
                    cur.push_str(&t);
                    has_text = true;
                    if strong_depth == 0 {
                        all_bold = false;
                    }
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if in_table {
                    table_cur_cell.push(' ');
                } else {
                    cur.push('\n');
                }
            }
            _ => {}
        }
    }
    flush(&mut blocks, &mut cur, cur_kind, &mut all_bold, &mut has_text);
    blocks
}

/// 文字列の表示幅を求める (全角=2, 半角=1)。等幅フォントでの桁揃えに使う (SPECv0.5.4 §5)。
fn display_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

/// 1文字の表示幅。CJK全角・かな・全角記号などは2、それ以外は1とする簡易判定。
fn char_width(c: char) -> usize {
    match c as u32 {
        // 全角スペース
        0x3000
        // CJK記号・かな・CJK統合漢字(拡張Aを含む)・全角英数記号・ハングル
        | 0x1100..=0x115F
        | 0x2E80..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6 => 2,
        _ => 1,
    }
}

/// 収集した行 (先頭がヘッダー) を等幅フォント向けの桁揃え文字列へ整形する。
/// 罫線は引かず `|` 区切り + スペースパディングの簡易表とし、ヘッダー下に区切り行を入れる。
fn format_table(rows: &[Vec<String>]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return String::new();
    }
    // 列ごとの最大表示幅を求める
    let mut widths = vec![0usize; cols];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(display_width(cell));
        }
    }
    let pad_cell = |cell: &str, w: usize| -> String {
        let pad = w.saturating_sub(display_width(cell));
        format!("{cell}{}", " ".repeat(pad))
    };
    let mut lines = Vec::new();
    for (ri, row) in rows.iter().enumerate() {
        let cells: Vec<String> = (0..cols)
            .map(|ci| {
                let cell = row.get(ci).map(String::as_str).unwrap_or("");
                pad_cell(cell, widths[ci])
            })
            .collect();
        lines.push(format!("| {} |", cells.join(" | ")));
        // ヘッダー行(先頭)の直後に区切り行を入れる
        if ri == 0 {
            let seps: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
            lines.push(format!("| {} |", seps.join(" | ")));
        }
    }
    lines.join("\n")
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

    #[test]
    fn parses_table_with_header_separator() {
        let md = "| Name | Age |\n|---|---|\n| Bob | 30 |\n| Alice | 5 |";
        let blocks = parse(md);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, BlockKind::Table);
        let lines: Vec<&str> = blocks[0].text.lines().collect();
        // ヘッダー + 区切り + 本文2行 = 4行
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0], "| Name  | Age |");
        assert_eq!(lines[1], "| ----- | --- |");
        assert_eq!(lines[2], "| Bob   | 30  |");
        assert_eq!(lines[3], "| Alice | 5   |");
    }

    #[test]
    fn table_aligns_by_display_width_for_japanese() {
        // 全角は幅2で数える。「氏名」(幅4) と「太郎」(幅4) が揃う。
        let md = "| 氏名 | 値 |\n|---|---|\n| 太郎 | 3 |";
        let blocks = parse(md);
        assert_eq!(blocks[0].kind, BlockKind::Table);
        let lines: Vec<&str> = blocks[0].text.lines().collect();
        assert_eq!(lines[0], "| 氏名 | 値 |");
        assert_eq!(lines[2], "| 太郎 | 3  |");
    }

    #[test]
    fn table_handles_ragged_rows() {
        // 列数が不揃いでも欠けたセルは空文字で埋める
        let md = "| A | B | C |\n|---|---|---|\n| 1 | 2 |";
        let blocks = parse(md);
        assert_eq!(blocks[0].kind, BlockKind::Table);
        let lines: Vec<&str> = blocks[0].text.lines().collect();
        assert_eq!(lines[2], "| 1 | 2 |   |");
    }

    #[test]
    fn display_width_counts_fullwidth_as_two() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("あいう"), 6);
        assert_eq!(display_width("A漢"), 3);
    }
}
