use serde_bytes::ByteBuf;
use serde_cbor::error;
use std;
use std::collections::{HashMap, VecDeque};
use std::convert::TryInto;
use std::path::{Path, PathBuf};

pub use serde_cbor::value::{from_value, Value};

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum LRValue {
    LValue,
    RValue,
}

impl LRValue {
    pub fn is_lvalue(&self) -> bool {
        *self == LRValue::LValue
    }
    pub fn is_rvalue(&self) -> bool {
        *self == LRValue::RValue
    }
}

#[derive(Copy, Debug, Clone, PartialOrd, PartialEq, Ord, Eq)]
pub struct SrcLoc {
    pub fileid: u64,
    pub line: u64,
    pub column: u64,
}

#[derive(Copy, Debug, Clone, PartialOrd, PartialEq, Ord, Eq)]
pub struct SrcSpan {
    pub fileid: u64,
    pub begin_line: u64,
    pub begin_column: u64,
    pub end_line: u64,
    pub end_column: u64,
}

impl From<SrcLoc> for SrcSpan {
    fn from(loc: SrcLoc) -> Self {
        Self {
            fileid: loc.fileid,
            begin_line: loc.line,
            begin_column: loc.column,
            end_line: loc.line,
            end_column: loc.column,
        }
    }
}

impl SrcSpan {
    pub fn begin(&self) -> SrcLoc {
        SrcLoc {
            fileid: self.fileid,
            line: self.begin_line,
            column: self.begin_column,
        }
    }
    pub fn end(&self) -> SrcLoc {
        SrcLoc {
            fileid: self.fileid,
            line: self.end_line,
            column: self.end_column,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AstNode {
    pub tag: ASTEntryTag,
    pub children: Vec<Option<u64>>,
    pub loc: SrcSpan,
    pub type_id: Option<u64>,
    pub rvalue: LRValue,

    // Stack of macros this node was expanded from, beginning with the initial
    // macro call and ending with the leaf. This needs to be a stack for nested
    // macro definitions.
    pub macro_expansions: Vec<u64>,
    pub macro_expansion_text: Option<String>,
    pub extras: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct TypeNode {
    pub tag: TypeTag,
    pub extras: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct CommentNode {
    pub loc: SrcLoc,
    pub string: String,
}

#[derive(Debug, Clone)]
pub struct SrcFile {
    pub path: Option<PathBuf>,
    pub include_loc: Option<SrcLoc>,
}

impl TypeNode {
    // Masks used to decode the IDs given to type nodes
    pub const ID_MASK: u64 = !0b111;
    pub const CONST_MASK: u64 = 0b001;
    pub const RESTRICT_MASK: u64 = 0b010;
    pub const VOLATILE_MASK: u64 = 0b100;
}

#[derive(Debug, Clone)]
pub struct AstContext {
    pub ast_nodes: HashMap<u64, AstNode>,
    pub type_nodes: HashMap<u64, TypeNode>,
    pub top_nodes: Vec<u64>,
    pub comments: Vec<CommentNode>,
    pub files: Vec<SrcFile>,
    pub va_list_kind: BuiltinVaListKind,
    pub target: String,
}

pub fn expect_opt_str(val: &Value) -> Option<Option<&str>> {
    match *val {
        Value::Null => Some(None),
        Value::Text(ref s) => Some(Some(s)),
        _ => None,
    }
}

pub fn expect_opt_u64(val: &Value) -> Option<Option<u64>> {
    match *val {
        Value::Null => Some(None),
        Value::Integer(n) => Some(Some(n.try_into().unwrap())),
        _ => None,
    }
}

fn import_ast_tag(tag: u64) -> ASTEntryTag {
    unsafe {
        return std::mem::transmute::<u32, ASTEntryTag>(tag as u32);
    }
}

fn import_type_tag(tag: u64) -> TypeTag {
    unsafe {
        return std::mem::transmute::<u32, TypeTag>(tag as u32);
    }
}

fn import_va_list_kind(tag: u64) -> BuiltinVaListKind {
    unsafe {
        return std::mem::transmute::<u32, BuiltinVaListKind>(tag as u32);
    }
}

pub fn process(items: Value) -> error::Result<AstContext> {
    let mut asts: HashMap<u64, AstNode> = HashMap::new();
    let mut types: HashMap<u64, TypeNode> = HashMap::new();
    let mut comments: Vec<CommentNode> = vec![];

    let (all_nodes, top_nodes, files, raw_comments, va_list_kind, target): (
        Vec<VecDeque<Value>>,
        Vec<u64>,
        Vec<(String, Option<(u64, u64, u64)>)>,
        Vec<(u64, u64, u64, ByteBuf)>,
        u64,
        String,
    ) = from_value(items)?;

    let va_list_kind = import_va_list_kind(va_list_kind);

    for (fileid, line, column, bytes) in raw_comments {
        comments.push(CommentNode {
            loc: SrcLoc {
                fileid,
                line,
                column,
            },
            string: String::from_utf8_lossy(&bytes).to_string(),
        })
    }

    let files = files
        .into_iter()
        .map(|(path, loc)| {
            let path = match path.as_str() {
                "" => None,
                "?" => None,
                path => Some(Path::new(path).to_path_buf()),
            };
            SrcFile {
                path,
                include_loc: loc.map(|(fileid, line, column)| SrcLoc {
                    fileid,
                    line,
                    column,
                }),
            }
        })
        .collect::<Vec<_>>();

    for mut entry in all_nodes.into_iter() {
        let entry_id: u64 = from_value(entry.pop_front().unwrap()).unwrap();
        let tag = from_value(entry.pop_front().unwrap()).unwrap();

        if tag < 400 {
            let children = from_value::<Vec<Value>>(entry.pop_front().unwrap())
                .unwrap()
                .iter()
                .map(|x| expect_opt_u64(x).unwrap())
                .collect::<Vec<Option<u64>>>();

            // entry[3]
            let fileid = from_value(entry.pop_front().unwrap()).unwrap();
            let begin_line = from_value(entry.pop_front().unwrap()).unwrap();
            let begin_column = from_value(entry.pop_front().unwrap()).unwrap();
            let end_line = from_value(entry.pop_front().unwrap()).unwrap();
            let end_column = from_value(entry.pop_front().unwrap()).unwrap();

            // entry[8]
            let type_id: Option<u64> = expect_opt_u64(&entry.pop_front().unwrap()).unwrap();

            // entry[9]
            let rvalue = if from_value(entry.pop_front().unwrap()).unwrap() {
                LRValue::RValue
            } else {
                LRValue::LValue
            };

            // entry[10]
            let macro_expansions = from_value::<Vec<u64>>(entry.pop_front().unwrap()).unwrap();

            let macro_expansion_text = expect_opt_str(&entry.pop_front().unwrap())
                .unwrap()
                .map(|s| s.to_string());

            let node = AstNode {
                tag: import_ast_tag(tag),
                children,
                loc: SrcSpan {
                    fileid,
                    begin_line,
                    begin_column,
                    end_line,
                    end_column,
                },
                type_id,
                rvalue,
                macro_expansions,
                macro_expansion_text,
                extras: entry.into_iter().collect(),
            };

            asts.insert(entry_id, node);
        } else {
            let node = TypeNode {
                tag: import_type_tag(tag),
                extras: entry.into_iter().collect(),
            };

            types.insert(entry_id, node);
        }
    }
    Ok(AstContext {
        top_nodes,
        ast_nodes: asts,
        type_nodes: types,
        comments,
        files,
        va_list_kind,
        target,
    })
}
