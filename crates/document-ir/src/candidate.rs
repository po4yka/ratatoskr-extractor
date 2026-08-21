use ratatoskr_document_contracts::DocumentBlock;

use crate::dom::{Element, HtmlDom};
use crate::{block, block_text_len, normalized_text};

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
    pub(crate) link_characters: usize,
    pub(crate) boilerplate_characters: usize,
}

pub(crate) fn extract(dom: &HtmlDom) -> [Candidate; 3] {
    [semantic(dom), readability(dom), density(dom)]
}

fn semantic(dom: &HtmlDom) -> Candidate {
    let root = ["article", "main"].into_iter().find_map(|name| {
        dom.elements()
            .filter(|element| element.name() == name)
            .find(|element| !blocks(*element).is_empty())
    });
    root.map_or_else(
        || empty(Strategy::Semantic),
        |root| candidate(Strategy::Semantic, root),
    )
}

fn readability(dom: &HtmlDom) -> Candidate {
    best_container(dom, Strategy::Readability, |text, _elements| text)
}

fn density(dom: &HtmlDom) -> Candidate {
    best_container(dom, Strategy::Density, |text, elements| {
        text.saturating_mul(1_000) / elements.max(1)
    })
}

fn best_container(
    dom: &HtmlDom,
    strategy: Strategy,
    score: impl Fn(usize, usize) -> usize,
) -> Candidate {
    dom.elements()
        .filter(|element| {
            matches!(
                element.name(),
                "article" | "main" | "section" | "div" | "body" | "html"
            )
        })
        .map(|element| {
            let candidate = candidate(strategy, element);
            let text = candidate
                .blocks
                .iter()
                .map(block_text_len)
                .sum::<usize>()
                .saturating_sub(candidate.link_characters)
                .saturating_sub(candidate.boilerplate_characters);
            let elements = element.elements().count();
            (score(text, elements), candidate)
        })
        .max_by_key(|(score, _candidate)| *score)
        .map_or_else(|| empty(strategy), |(_score, candidate)| candidate)
}

fn blocks(root: Element<'_>) -> Vec<DocumentBlock> {
    root.elements().filter_map(block).collect()
}

fn candidate(strategy: Strategy, root: Element<'_>) -> Candidate {
    Candidate {
        strategy,
        blocks: blocks(root),
        link_characters: root
            .elements()
            .filter(|element| element.name() == "a")
            .filter_map(normalized_text)
            .map(|text| text.len())
            .sum(),
        boilerplate_characters: root
            .elements()
            .filter(|element| {
                matches!(
                    element.name(),
                    "aside" | "footer" | "form" | "header" | "nav"
                )
            })
            .flat_map(blocks)
            .map(|block| block_text_len(&block))
            .sum(),
    }
}

fn empty(strategy: Strategy) -> Candidate {
    Candidate {
        strategy,
        blocks: Vec::new(),
        link_characters: 0,
        boilerplate_characters: 0,
    }
}
