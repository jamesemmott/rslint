//! Extremely fast, lossless, and error tolerant JavaScript Parser.
//!
//! The parser uses an abstraction over non-whitespace tokens.
//! This allows us to losslessly or lossly parse code without requiring explicit handling of whitespace.
//! The parser yields events, not an AST, the events are resolved into untyped syntax nodes, which can then
//! be casted into a typed AST.
//!
//! The parser is able to produce a valid AST from **any** source code.
//! Erroneous productions are wrapped into `ERROR` syntax nodes, the original source code
//! is completely represented in the final syntax nodes.
//!
//! You probably do not want to use the parser struct, unless you want to parse fragments of Js source code or make your own productions.
//! Instead use functions such as [`parse_text`] and [`parse_text_lossy`] which offer abstracted versions for parsing.
//!
//! Notable features of the parser are:
//! - Extremely fast parsing and lexing through the extremely fast [`rslint_lexer`].
//! - Ability to do Lossy or Lossless parsing on demand without explicit whitespace handling.
//! - Customizable, able to parse any fragments of JS code at your discretion.
//! - Completely error tolerant, able to produce an AST from any source code.
//! - Zero cost for converting untyped nodes to a typed AST.
//! - Ability to go from AST to SyntaxNodes to SyntaxTokens to source code and back very easily with nearly zero cost.
//! - Very easy tree traversal through [`SyntaxNode`](rslint_rowan::SyntaxNode).
//! - Descriptive errors with multiple labels and notes.
//! - Very cheap cloning, cloning an ast node or syntax node is the cost of adding a reference to an Rc.
//! - Cheap incremental reparsing of changed text.
//!
//! The crate further includes utilities such as:
//! - ANSI syntax highlighting of nodes (through [`util`]) or text through [`rslint_lexer`].
//! - Rich utility functions for syntax nodes through [`SyntaxNodeExt`].
//!
//! It is inspired by the rust analyzer parser but adapted for JavaScript.
//!
//! # Syntax Nodes vs AST Nodes
//! The crate relies on a concept of untyped [`SyntaxNode`]s vs typed [`AstNode`]s.
//! Syntax nodes represent the syntax tree in an untyped way. They represent a location in an immutable
//! tree with two pointers. The syntax tree is composed of [`SyntaxNode`]s and [`SyntaxToken`]s in a nested
//! tree structure. Each node can have parents, siblings, children, descendants, etc.  
//!
//! [`AstNode`]s represent a typed version of a syntax node. They have the same exact representation as syntax nodes
//! therefore a conversion between either has zero runtime cost. Every piece of data of an ast node is optional,
//! this is due to the fact that the parser is completely error tolerant.
//!
//! Each representation has its advantages:
//!
//! ### SyntaxNodes
//! - Very simple traversing of the syntax tree through functions on them.
//! - Easily able to convert to underlying text, range, or tokens.
//! - Contain all whitespace bound to the underlying production (in the case of lossless parsing).
//! - Can be easily converted into its typed representation with zero cost.
//! - Can be turned into a pretty representation with fmt debug.
//!
//! ### AST Nodes
//! - Easy access to properties of the underlying production.
//! - Zero cost conversion to a syntax node.
//!
//! In conclusion, the use of both representations means we are not constrained to acting through
//! typed nodes. Which makes traversal hard and you often have to resort to autogenerated visitor patterns.
//! AST nodes are simply a way to easily access subproperties of a syntax node.event;
mod parser;
#[macro_use]
mod token_set;
mod event;
mod lossless_tree_sink;
mod lossy_tree_sink;
mod numbers;
mod parse;
mod state;
mod syntax_node;
mod token_source;

#[cfg(test)]
mod tests;

#[macro_use]
pub mod ast;
pub mod syntax;
pub mod util;

pub use crate::{
    ast::{AstNode, AstToken},
    event::{process, Event},
    lossless_tree_sink::LosslessTreeSink,
    lossy_tree_sink::LossyTreeSink,
    numbers::{parse_js_num, BigInt, JsNum},
    parse::*,
    parser::{Checkpoint, CompletedMarker, Marker, Parser},
    state::{ParserState, StrictMode},
    syntax_node::*,
    token_set::TokenSet,
    token_source::TokenSource,
    util::{SyntaxNodeExt, SyntaxTokenExt},
};

pub use rslint_rowan::{SmolStr, SyntaxText, TextRange, TextSize, WalkEvent};

pub use rslint_syntax::*;

/// The type of error emitted by the parser, this includes warnings, notes, and errors.  
/// It also includes labels and possibly notes
pub type ParserError = rslint_errors::Diagnostic;

use rslint_lexer::color;
use std::ops::Range;

/// Abstracted token for `TokenSource`
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct Token {
    /// What kind of token it is
    pub kind: SyntaxKind,
    /// The range (in byte indices) of the token
    pub range: Range<usize>,
    /// How long the token is
    pub len: TextSize,
}

impl From<Token> for Range<usize> {
    fn from(token: Token) -> Self {
        token.range
    }
}

/// An abstraction for syntax tree implementations
pub trait TreeSink {
    /// Adds new token to the current branch.
    fn token(&mut self, kind: SyntaxKind);

    /// Start new branch and make it current.
    fn start_node(&mut self, kind: SyntaxKind);

    /// Finish current branch and restore previous
    /// branch as current.
    fn finish_node(&mut self);

    /// Emit an error
    fn error(&mut self, error: ParserError);

    /// Consume multiple tokens and glue them into one kind
    fn consume_multiple_tokens(&mut self, amount: u8, kind: SyntaxKind);
}

/// Matches a `SyntaxNode` against an `ast` type.
///
/// # Example:
///
/// ```ignore
/// match_ast! {
///     match node {
///         ast::CallExpr(it) => { ... },
///         ast::BlockStmt(it) => { ... },
///         ast::Script(it) => { ... },
///         _ => None,
///     }
/// }
/// ```
#[macro_export]
macro_rules! match_ast {
    (match $node:ident { $($tt:tt)* }) => { match_ast!(match ($node) { $($tt)* }) };

    (match ($node:expr) {
        $( ast::$ast:ident($it:ident) => $res:expr, )*
        _ => $catch_all:expr $(,)?
    }) => {{
        $( if let Some($it) = ast::$ast::cast($node.clone()) { $res } else )*
        { $catch_all }
    }};
}

/// A structure describing the syntax features the parser will accept. The
/// default is an ECMAScript 2021 Script without any proposals.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Syntax {
    pub file_kind: FileKind,
    pub top_level_await: bool,
    pub global_return: bool,
    pub class_fields: bool,
    pub decorators: bool,
}

impl Syntax {
    pub fn new(file_kind: FileKind) -> Self {
        let mut this = Self {
            file_kind,
            ..Default::default()
        };
        if file_kind == FileKind::TypeScript {
            this = this.typescript();
        }
        this
    }

    pub fn top_level_await(mut self) -> Self {
        self.top_level_await = true;
        self
    }

    pub fn global_return(mut self) -> Self {
        self.global_return = true;
        self
    }

    pub fn class_fields(mut self) -> Self {
        self.class_fields = true;
        self
    }

    pub fn decorators(mut self) -> Self {
        self.decorators = true;
        self
    }

    pub fn script(mut self) -> Self {
        self.file_kind = FileKind::Script;
        self
    }

    pub fn module(mut self) -> Self {
        self.file_kind = FileKind::Module;
        self
    }

    pub fn typescript(mut self) -> Self {
        self.file_kind = FileKind::TypeScript;
        self.class_fields = true;
        self.decorators = true;
        self
    }
}

/// The kind of file we are parsing
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileKind {
    Script,
    Module,
    TypeScript,
}

impl Default for FileKind {
    fn default() -> Self {
        FileKind::Script
    }
}

impl From<FileKind> for Syntax {
    fn from(kind: FileKind) -> Self {
        Syntax::new(kind)
    }
}
