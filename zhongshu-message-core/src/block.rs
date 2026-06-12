use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MessageBlock {
    Paragraph {
        text: String,
    },
    Code {
        language: Option<String>,
        content: String,
    },
    Heading {
        level: u8,
        text: String,
    },
    List {
        ordered: bool,
        items: Vec<String>,
    },
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    Blockquote {
        text: String,
    },
    ThematicBreak,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct BlockTree {
    pub blocks: Vec<MessageBlock>,
}

impl BlockTree {
    pub fn new(blocks: Vec<MessageBlock>) -> Self {
        BlockTree { blocks }
    }

    pub fn empty() -> Self {
        BlockTree { blocks: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }
}
