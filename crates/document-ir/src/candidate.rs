use ratatoskr_document_contracts::DocumentBlock;

use crate::dom::{Element, HtmlDom};
use crate::{block, block_text_len};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Strategy {
    Semantic,
    Readability,
    Density,
}

impl Strategy {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Semantic => "semantic",
            Self::Readability => "readability",
            Self::Density => "density",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Candidate {
    pub(crate) strategy: Strategy,
    pub(crate) blocks: Vec<DocumentBlock>,
}

pub(crate) fn extract(dom: &HtmlDom) -> [Candidate; 3] {
    [semantic(dom), readability(dom), density(dom)]
}

fn semantic(dom: &HtmlDom) -> Candidate {
    let blocks = ["article", "main"]
        .into_iter()
        .find_map(|name| {
            dom.elements()
                .filter(|element| element.name() == name)
                .map(blocks)
                .find(|blocks| !blocks.is_empty())
        })
        .unwrap_or_default();
    Candidate {
        strategy: Strategy::Semantic,
        blocks,
    }
}

fn readability(dom: &HtmlDom) -> Candidate {
    Candidate {
        strategy: Strategy::Readability,
        blocks: best_container(dom, |text, _elements| text),
    }
}

fn density(dom: &HtmlDom) -> Candidate {
    Candidate {
        strategy: Strategy::Density,
        blocks: best_container(dom, |text, elements| {
            text.saturating_mul(1_000) / elements.max(1)
        }),
    }
}

fn best_container(dom: &HtmlDom, score: impl Fn(usize, usize) -> usize) -> Vec<DocumentBlock> {
    dom.elements()
        .filter(|element| {
            matches!(
                element.name(),
                "article" | "main" | "section" | "div" | "body" | "html"
            )
        })
        .map(|element| {
            let blocks = blocks(element);
            let text = blocks.iter().map(block_text_len).sum();
            let elements = element.elements().count();
            (score(text, elements), blocks)
        })
        .max_by_key(|(score, _blocks)| *score)
        .map_or_else(Vec::new, |(_score, blocks)| blocks)
}

fn blocks(root: Element<'_>) -> Vec<DocumentBlock> {
    root.elements().filter_map(block).collect()
}
