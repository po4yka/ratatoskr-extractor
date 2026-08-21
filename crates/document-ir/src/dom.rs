use std::borrow::Cow;
use std::cell::{Ref, RefCell};

use ego_tree::{NodeId, NodeRef, Tree};
use html5ever::tendril::{StrTendril, TendrilSink as _};
use html5ever::tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::{Attribute, ParseOpts, QualName, driver, expanded_name, local_name, ns};

#[derive(Debug)]
pub(super) struct HtmlDom {
    tree: Tree<Node>,
}

impl HtmlDom {
    pub(super) fn parse(source: &str) -> Self {
        driver::parse_document(DomSink::new(Self::new()), ParseOpts::default()).one(source)
    }

    pub(super) fn node_count(&self) -> usize {
        self.tree.nodes().count()
    }

    pub(super) fn elements(&self) -> impl Iterator<Item = Element<'_>> {
        self.tree.nodes().filter_map(Element::wrap)
    }

    fn new() -> Self {
        Self {
            tree: Tree::new(Node::new(NodeKind::Document)),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Element<'a> {
    node: NodeRef<'a, Node>,
}

impl<'a> Element<'a> {
    fn wrap(node: NodeRef<'a, Node>) -> Option<Self> {
        matches!(node.value().kind, NodeKind::Element { .. }).then_some(Self { node })
    }

    pub(super) fn name(self) -> &'a str {
        self.node.value().name.local.as_ref()
    }

    pub(super) fn attr(self, name: &str) -> Option<&'a str> {
        match &self.node.value().kind {
            NodeKind::Element { attrs } => attrs
                .iter()
                .find(|attribute| attribute.name.local.as_ref() == name)
                .map(|attribute| attribute.value.as_ref()),
            _ => None,
        }
    }

    pub(super) fn text(self) -> impl Iterator<Item = &'a str> {
        self.node.traverse().filter_map(|edge| match edge {
            ego_tree::iter::Edge::Open(node) => match &node.value().kind {
                NodeKind::Text(text) => Some(text.as_ref()),
                _ => None,
            },
            ego_tree::iter::Edge::Close(_) => None,
        })
    }

    pub(super) fn elements(self) -> impl Iterator<Item = Self> {
        std::iter::once(self).chain(self.node.descendants().filter_map(Self::wrap))
    }
}

#[derive(Debug)]
struct Node {
    name: QualName,
    kind: NodeKind,
}

impl Node {
    fn new(kind: NodeKind) -> Self {
        Self {
            name: QualName::new(None, ns!(), local_name!("")),
            kind,
        }
    }

    fn element(name: QualName, attrs: Vec<Attribute>) -> Self {
        Self {
            name,
            kind: NodeKind::Element { attrs },
        }
    }
}

#[derive(Debug)]
enum NodeKind {
    Document,
    Fragment,
    Element { attrs: Vec<Attribute> },
    Text(StrTendril),
    Other,
}

#[derive(Debug)]
struct DomSink(RefCell<HtmlDom>);

impl DomSink {
    fn new(dom: HtmlDom) -> Self {
        Self(RefCell::new(dom))
    }
}

impl TreeSink for DomSink {
    type Handle = NodeId;
    type Output = HtmlDom;
    type ElemName<'a> = Ref<'a, QualName>;

    fn finish(self) -> Self::Output {
        self.0.into_inner()
    }

    fn parse_error(&self, _message: Cow<'static, str>) {}

    fn get_document(&self) -> Self::Handle {
        self.0.borrow().tree.root().id()
    }

    fn elem_name<'a>(&'a self, target: &'a Self::Handle) -> Self::ElemName<'a> {
        Ref::map(self.0.borrow(), |dom| {
            dom.tree
                .get(*target)
                .map_or(&dom.tree.root().value().name, |node| &node.value().name)
        })
    }

    fn create_element(
        &self,
        name: QualName,
        attrs: Vec<Attribute>,
        _flags: ElementFlags,
    ) -> Self::Handle {
        let template = name.expanded() == expanded_name!(html "template");
        let mut dom = self.0.borrow_mut();
        let mut node = dom.tree.orphan(Node::element(name, attrs));
        if template {
            node.append(Node::new(NodeKind::Fragment));
        }
        node.id()
    }

    fn create_comment(&self, _text: StrTendril) -> Self::Handle {
        self.0
            .borrow_mut()
            .tree
            .orphan(Node::new(NodeKind::Other))
            .id()
    }

    fn create_pi(&self, _target: StrTendril, _data: StrTendril) -> Self::Handle {
        self.0
            .borrow_mut()
            .tree
            .orphan(Node::new(NodeKind::Other))
            .id()
    }

    fn append(&self, parent: &Self::Handle, child: NodeOrText<Self::Handle>) {
        let mut dom = self.0.borrow_mut();
        let Some(mut parent) = dom.tree.get_mut(*parent) else {
            return;
        };
        match child {
            NodeOrText::AppendNode(node) => {
                parent.append_id(node);
            }
            NodeOrText::AppendText(text) => {
                let joined = parent.last_child().is_some_and(|mut child| {
                    if let NodeKind::Text(existing) = &mut child.value().kind {
                        existing.push_tendril(&text);
                        true
                    } else {
                        false
                    }
                });
                if !joined {
                    parent.append(Node::new(NodeKind::Text(text)));
                }
            }
        }
    }

    fn append_based_on_parent_node(
        &self,
        element: &Self::Handle,
        previous: &Self::Handle,
        child: NodeOrText<Self::Handle>,
    ) {
        let has_parent = self
            .0
            .borrow()
            .tree
            .get(*element)
            .is_some_and(|node| node.parent().is_some());
        if has_parent {
            self.append_before_sibling(element, child);
        } else {
            self.append(previous, child);
        }
    }

    fn append_doctype_to_document(
        &self,
        _name: StrTendril,
        _public_id: StrTendril,
        _system_id: StrTendril,
    ) {
        self.0
            .borrow_mut()
            .tree
            .root_mut()
            .append(Node::new(NodeKind::Other));
    }

    fn get_template_contents(&self, target: &Self::Handle) -> Self::Handle {
        self.0
            .borrow()
            .tree
            .get(*target)
            .and_then(|node| node.first_child())
            .map_or_else(|| self.get_document(), |node| node.id())
    }

    fn same_node(&self, left: &Self::Handle, right: &Self::Handle) -> bool {
        left == right
    }

    fn set_quirks_mode(&self, _mode: QuirksMode) {}

    fn append_before_sibling(&self, sibling: &Self::Handle, child: NodeOrText<Self::Handle>) {
        let mut dom = self.0.borrow_mut();
        if let NodeOrText::AppendNode(node) = child
            && let Some(mut target) = dom.tree.get_mut(node)
        {
            target.detach();
        }
        let Some(mut sibling) = dom.tree.get_mut(*sibling) else {
            return;
        };
        if sibling.parent().is_none() {
            return;
        }
        match child {
            NodeOrText::AppendNode(node) => {
                sibling.insert_id_before(node);
            }
            NodeOrText::AppendText(text) => {
                let joined = sibling.prev_sibling().is_some_and(|mut previous| {
                    if let NodeKind::Text(existing) = &mut previous.value().kind {
                        existing.push_tendril(&text);
                        true
                    } else {
                        false
                    }
                });
                if !joined {
                    sibling.insert_before(Node::new(NodeKind::Text(text)));
                }
            }
        }
    }

    fn add_attrs_if_missing(&self, target: &Self::Handle, attrs: Vec<Attribute>) {
        let mut dom = self.0.borrow_mut();
        let Some(mut node) = dom.tree.get_mut(*target) else {
            return;
        };
        let NodeKind::Element { attrs: existing } = &mut node.value().kind else {
            return;
        };
        for attribute in attrs {
            if !existing
                .iter()
                .any(|current| current.name == attribute.name)
            {
                existing.push(attribute);
            }
        }
    }

    fn remove_from_parent(&self, target: &Self::Handle) {
        if let Some(mut node) = self.0.borrow_mut().tree.get_mut(*target) {
            node.detach();
        }
    }

    fn reparent_children(&self, node: &Self::Handle, new_parent: &Self::Handle) {
        if let Some(mut parent) = self.0.borrow_mut().tree.get_mut(*new_parent) {
            parent.reparent_from_id_append(*node);
        }
    }
}
