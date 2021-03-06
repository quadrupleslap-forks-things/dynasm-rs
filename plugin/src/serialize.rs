use std::rc::Rc;

use byteorder::{ByteOrder, LittleEndian};

use syntax::util::ThinVec;
use syntax::ext::build::AstBuilder;
use syntax::ext::base::ExtCtxt;
use syntax::ast;
use syntax::ptr::P;
use syntax::symbol::Symbol;
use syntax::codemap::{Span, Spanned};

pub type Ident = Spanned<ast::Ident>;

#[derive(Debug, PartialOrd, PartialEq, Ord, Eq, Hash, Clone, Copy)]
pub enum Size {
    BYTE  = 1,
    WORD  = 2,
    DWORD = 4,
    FWORD = 6,
    QWORD = 8,
    PWORD = 10,
    OWORD = 16,
    HWORD = 32,
}

impl Size {
    pub fn in_bytes(&self) -> u8 {
        *self as u8
    }

    pub fn as_literal(&self) -> ast::Ident {
        ast::Ident::from_str(match *self {
            Size::BYTE  => "i8",
            Size::WORD  => "i16",
            Size::DWORD => "i32",
            Size::FWORD => "i48",
            Size::QWORD => "i64",
            Size::PWORD => "i80",
            Size::OWORD => "i128",
            Size::HWORD => "i256"
        })
    }
}

#[derive(Debug, Clone)]
pub enum Stmt {
    // simply push data into the instruction stream. unsigned
    Const(u64, Size),
    // push data that is stored inside of an expression. unsigned
    ExprUnsigned(P<ast::Expr>, Size),
    // push signed data into the instruction stream. signed
    ExprSigned(P<ast::Expr>, Size),

    // extend the instruction stream with unsigned bytes
    Extend(Vec<u8>),
    // extend the instruction stream with unsigned bytes
    ExprExtend(P<ast::Expr>),
    // align the instruction stream to some alignment
    Align(P<ast::Expr>),

    // label declarations
    GlobalLabel(Ident),
    LocalLabel(Ident),
    DynamicLabel(P<ast::Expr>),

    // and their respective relocations (as expressions as they differ per assembler)
    GlobalJumpTarget(Ident,         P<ast::Expr>),
    ForwardJumpTarget(Ident,        P<ast::Expr>),
    BackwardJumpTarget(Ident,       P<ast::Expr>),
    DynamicJumpTarget(P<ast::Expr>, P<ast::Expr>),
    BareJumpTarget(   P<ast::Expr>, P<ast::Expr>),

    // a random statement that has to be inserted between assembly hunks
    Stmt(ast::Stmt)
}

// convenience methods
impl Stmt {
    pub fn u8(value: u8) -> Stmt {
        Stmt::Const(value as u64, Size::BYTE)
    }

    pub fn u16(value: u16) -> Stmt {
        Stmt::Const(value as u64, Size::WORD)
    }

    pub fn u32(value: u32) -> Stmt {
        Stmt::Const(value as u64, Size::DWORD)
    }

    pub fn u64(value: u64) -> Stmt {
        Stmt::Const(value, Size::QWORD)
    }
}

pub fn serialize(ecx: &mut ExtCtxt, name: P<ast::Expr>, stmts: Vec<Stmt>) -> Vec<ast::Stmt> {
    // first, try to fold constants into a byte stream
    let mut folded_stmts = Vec::new();
    let mut const_buffer = Vec::new();
    for stmt in stmts {
        match stmt {
            Stmt::Const(value, size) => {
                match size {
                    Size::BYTE => const_buffer.push(value as u8),
                    Size::WORD => {
                        let mut buffer = [0u8; 2];
                        LittleEndian::write_u16(&mut buffer, value as u16);
                        const_buffer.extend(&buffer);
                    },
                    Size::DWORD => {
                        let mut buffer = [0u8; 4];
                        LittleEndian::write_u32(&mut buffer, value as u32);
                        const_buffer.extend(&buffer);
                    },
                    Size::QWORD => {
                        let mut buffer = [0u8; 8];
                        LittleEndian::write_u64(&mut buffer, value as u64);
                        const_buffer.extend(&buffer);
                    },
                    _ => unimplemented!()
                }
            },
            Stmt::Extend(data) => {
                const_buffer.extend(data);
            },
            s => {
                // empty the const buffer
                if !const_buffer.is_empty() {
                    folded_stmts.push(Stmt::Extend(const_buffer));
                    const_buffer = Vec::new();
                }
                folded_stmts.push(s);
            }
        }
        while const_buffer.len() > 32 {
            let new_buffer = const_buffer.split_off(32);
            folded_stmts.push(Stmt::Extend(const_buffer));
            const_buffer = new_buffer;
        }
    }
    if !const_buffer.is_empty() {
        folded_stmts.push(Stmt::Extend(const_buffer));
    }

    // and now do the final output pass in one go
    let mut output = Vec::new();

    for stmt in folded_stmts {
        let (method, args) = match stmt {
            Stmt::Const(_, _) => unreachable!(),
            Stmt::ExprUnsigned(expr, Size::BYTE)  => ("push",     vec![expr]),
            Stmt::ExprUnsigned(expr, Size::WORD)  => ("push_u16", vec![expr]),
            Stmt::ExprUnsigned(expr, Size::DWORD) => ("push_u32", vec![expr]),
            Stmt::ExprUnsigned(expr, Size::QWORD) => ("push_u64", vec![expr]),
            Stmt::ExprUnsigned(_, _) => unimplemented!(),
            Stmt::ExprSigned(  expr, Size::BYTE)  => ("push_i8",  vec![expr]),
            Stmt::ExprSigned(  expr, Size::WORD)  => ("push_i16", vec![expr]),
            Stmt::ExprSigned(  expr, Size::DWORD) => ("push_i32", vec![expr]),
            Stmt::ExprSigned(  expr, Size::QWORD) => ("push_i64", vec![expr]),
            Stmt::ExprSigned(_, _) => unimplemented!(),
            Stmt::Extend(data)     => ("extend", vec![ecx.expr_lit(ecx.call_site(), ast::LitKind::ByteStr(Rc::new(data)))]),
            Stmt::ExprExtend(expr) => ("extend", vec![expr]),
            Stmt::Align(expr)      => ("align", vec![expr]),
            Stmt::GlobalLabel(n) => ("global_label", vec![expr_string_from_ident(ecx, n)]),
            Stmt::LocalLabel(n)  => ("local_label", vec![expr_string_from_ident(ecx, n)]),
            Stmt::DynamicLabel(expr) => ("dynamic_label", vec![expr]),
            Stmt::GlobalJumpTarget(n,     reloc) => ("global_reloc"  , vec![expr_string_from_ident(ecx, n), reloc]),
            Stmt::ForwardJumpTarget(n,    reloc) => ("forward_reloc" , vec![expr_string_from_ident(ecx, n), reloc]),
            Stmt::BackwardJumpTarget(n,   reloc) => ("backward_reloc", vec![expr_string_from_ident(ecx, n), reloc]),
            Stmt::DynamicJumpTarget(expr, reloc) => ("dynamic_reloc" , vec![expr, reloc]),
            Stmt::BareJumpTarget(expr, reloc)    => ("bare_reloc"    , vec![expr, reloc]),
            Stmt::Stmt(s) => {
                output.push(s);
                continue;
            }

        };

        // and construct the appropriate method call
        let op = name.clone();
        let method = ast::Ident::from_str(method);
        let expr = ecx.expr_method_call(ecx.call_site(), op, method, args);
        output.push(ecx.stmt_semi(expr));
    }

    output
}

// below here are all kinds of utility functions to quickly generate ast constructs
// this collection is very arbitrary, purely what's needed in codegen, trying to keep
// most expression building logic in this file.

// expression of value 0. sometimes needed.
pub fn expr_zero(ecx: &ExtCtxt) -> P<ast::Expr> {
    ecx.expr_lit(ecx.call_site(), ast::LitKind::Int(0u128, ast::LitIntType::Unsuffixed))
}

// given an ident, makes it into a "string"
pub fn expr_string_from_ident(ecx: &ExtCtxt, i: Ident) -> P<ast::Expr> {
    ecx.expr_lit(i.span, ast::LitKind::Str(i.node.name, ast::StrStyle::Cooked))
}

// 
pub fn expr_dynscale(ecx: &ExtCtxt, name: &P<ast::Expr>, scale: P<ast::Expr>, rest: P<ast::Expr>) -> (ast::Stmt, P<ast::Expr>) {
    let temp = ast::Ident::from_str("temp");
    (
        ecx.stmt_let(ecx.call_site(), false, temp, expr_encode_x64_sib_scale(ecx, &name, scale)),
        expr_mask_shift_or(ecx, rest, ecx.expr_ident(ecx.call_site(), temp), 3, 6)
    )
}

// makes (a, b)
pub fn expr_tuple_of_u8s(ecx: &ExtCtxt, span: Span, data: &[u8]) -> P<ast::Expr> {
    ecx.expr_tuple(
        span,
        data.iter().map(|&a| ecx.expr_u8(span, a)).collect()
    )
}

// makes sum(exprs)
pub fn expr_add_many<T: Iterator<Item=P<ast::Expr>>>(ecx: &ExtCtxt, span: Span, mut exprs: T) -> Option<P<ast::Expr>> {
    exprs.next().map(|mut accum| {
        for next in exprs {
            accum = ecx.expr_binary(span, ast::BinOpKind::Add, accum, next);
        }
        accum
    })
}

// makes (size_of<ty>() * value)
pub fn expr_size_of_scale(ecx: &ExtCtxt, ty: ast::Path, value: P<ast::Expr>, size: Size) -> P<ast::Expr> {
    let span = value.span;
    ecx.expr_binary(span,
        ast::BinOpKind::Mul,
        ecx.expr_cast(span,
            expr_size_of(ecx, ty),
            ecx.ty_ident(span, size.as_literal())
        ),
        value
    )
}

/// returns orig | ((expr & mask) << shift)
pub fn expr_mask_shift_or(ecx: &ExtCtxt, orig: P<ast::Expr>, mut expr: P<ast::Expr>, mask: u64, shift: i8) -> P<ast::Expr> {
    let span = expr.span;

    expr = ecx.expr_binary(span, ast::BinOpKind::BitAnd, expr, ecx.expr_lit(
        span, ast::LitKind::Int(mask as u128, ast::LitIntType::Unsuffixed)
    ));

    if shift < 0 {
        expr = ecx.expr_binary(span, ast::BinOpKind::Shr, expr, ecx.expr_lit(
            span, ast::LitKind::Int((-shift) as u128, ast::LitIntType::Unsuffixed)
        ));
    } else if shift > 0 {
        expr = ecx.expr_binary(span, ast::BinOpKind::Shl, expr, ecx.expr_lit(
            span, ast::LitKind::Int(shift as u128, ast::LitIntType::Unsuffixed)
        ));
    }

    ecx.expr_binary(span, ast::BinOpKind::BitOr, orig, expr)
}

/// returns (offset_of!(path, attr) as size)
pub fn expr_offset_of(ecx: &ExtCtxt, path: ast::Path, attr: ast::Ident, size: Size) -> P<ast::Expr> {
    // generate a P<Expr> that resolves into the offset of an attribute to a type.
    // this is somewhat ridiculously complex because we can't expand macros here

    let span = path.span;

    let structpat = ecx.pat_struct(span, path.clone(), vec![
        Spanned {span: span, node: ast::FieldPat {
            attrs: ThinVec::new(),
            ident: attr,
            pat: ecx.pat_wild(span),
            is_shorthand: false
        }},
    ]).map(|mut pat| {
        if let ast::PatKind::Struct(_, _, ref mut dotdot) = pat.node {
            *dotdot = true;
        }
        pat
    });

    // there's no default constructor function for let pattern;
    let validation_stmt = ast::Stmt {
        id: ast::DUMMY_NODE_ID,
        span: span,
        node: ast::StmtKind::Local(P(ast::Local {
            pat: structpat,
            ty: None,
            init: None,
            id: ast::DUMMY_NODE_ID,
            span: span,
            attrs: ast::ThinVec::new()
        }))
    };

    let temp     = ast::Ident::from_str("temp");
    let rv       = ast::Ident::from_str("rv");
    let usize_id = ast::Ident::from_str("usize");
    let uninitialized = ["std", "mem", "uninitialized"].iter().cloned().map(ast::Ident::from_str).collect();
    let forget        = ["std", "mem", "forget"       ].iter().cloned().map(ast::Ident::from_str).collect();

    // unsafe {
    let block = ecx.block(span, vec![
        // let path { attr: _, ..};
        validation_stmt,
        // let temp: path = ::std::mem::uninitialized();
        ecx.stmt_let_typed(span, false, temp, ecx.ty_path(path),
            ecx.expr_call_global(span, uninitialized, Vec::new())
        ),
        // let rv = &temp.attr as *const _ as usize - &temp as *const _ as usize;
        ecx.stmt_let(span,
            false,
            rv,
            ecx.expr_binary(span, ast::BinOpKind::Sub,
                ecx.expr_cast(span,
                    ecx.expr_cast(span,
                        ecx.expr_addr_of(span,
                            ecx.expr_field_access(span,
                                ecx.expr_ident(span, temp),
                                attr
                            )
                        ), ecx.ty_ptr(span, ecx.ty_infer(span), ast::Mutability::Immutable)
                    ), ecx.ty_ident(span, usize_id)
                ),
                ecx.expr_cast(span,
                    ecx.expr_cast(span,
                        ecx.expr_addr_of(span, ecx.expr_ident(span, temp)),
                        ecx.ty_ptr(span, ecx.ty_infer(span), ast::Mutability::Immutable)
                    ), ecx.ty_ident(span, usize_id)
                )
            )
        ),
        // ::std::mem::forget(temp);
        ecx.stmt_semi(ecx.expr_call_global(span, forget, vec![ecx.expr_ident(span, temp)])),
        // rv as i32
        ecx.stmt_expr(ecx.expr_cast(span, ecx.expr_ident(span, rv), ecx.ty_ident(span, size.as_literal())))
    ]).map(|mut b| {
        b.rules = ast::BlockCheckMode::Unsafe(ast::UnsafeSource::CompilerGenerated);
        b
    });

    ecx.expr_block(block)
}

// returns std::mem::size_of<path>()
pub fn expr_size_of(ecx: &ExtCtxt, path: ast::Path) -> P<ast::Expr> {
    // generate a P<Expr> that returns the size of type at path
    let span = path.span;

    let ty = ecx.ty_path(path);
    let idents = ["std", "mem", "size_of"].iter().cloned().map(ast::Ident::from_str).collect();
    let size_of = ecx.path_all(span, true, idents, vec![ast::GenericArg::Type(ty)], Vec::new());
    ecx.expr_call(span, ecx.expr_path(size_of), Vec::new())
}

// makes the following
// match size {
//    8 => 3,
//    4 => 2,
//    2 => 1,
//    1 => 0,
//  _ => name.runtime_error("Type size not representable as scale")
//}
pub fn expr_encode_x64_sib_scale(ecx: &ExtCtxt, name: &P<ast::Expr>, size: P<ast::Expr>) -> P<ast::Expr> {
    let span = size.span;

    ecx.expr_match(span, size, vec![
        ecx.arm(span, vec![ecx.pat_lit(span, ecx.expr_usize(span, 8))], ecx.expr_u8(span, 3)),
        ecx.arm(span, vec![ecx.pat_lit(span, ecx.expr_usize(span, 4))], ecx.expr_u8(span, 2)),
        ecx.arm(span, vec![ecx.pat_lit(span, ecx.expr_usize(span, 2))], ecx.expr_u8(span, 1)),
        ecx.arm(span, vec![ecx.pat_lit(span, ecx.expr_usize(span, 1))], ecx.expr_u8(span, 0)),
        ecx.arm(span, vec![ecx.pat_wild(span)], ecx.expr_method_call(span,
            name.clone(),
            ast::Ident::from_str("runtime_error"),
            vec![ecx.expr_str(span,
                Symbol::intern("Type size not representable as scale")
            )]
        ))
    ])
}
