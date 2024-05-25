use serde::Serialize;

use crate::{NixSession, Value};

#[derive(Clone)]
pub struct NixExprBuilder {
	pub(crate) out: String,
	used_fields: Vec<Value>,
}
impl NixExprBuilder {
	pub fn object() -> Self {
		NixExprBuilder {
			out: "{ ".to_owned(),
			used_fields: Vec::new(),
		}
	}
	pub fn string(s: &str) -> Self {
		NixExprBuilder {
			out: nixlike::serialize(s)
				.expect("no problems with serializing_string")
				.trim_end()
				.to_owned(),
			used_fields: Vec::new(),
		}
	}
	pub fn serialized(v: impl Serialize) -> Self {
		let serialized = nixlike::serialize(v).expect("invalid value for apply");
		Self {
			out: serialized.trim_end().to_owned(),
			used_fields: Vec::new(),
		}
	}
	pub fn value(f: Value) -> Self {
		Self {
			out: format!("sess_field_{}", f.session_field_id()),
			used_fields: vec![f],
		}
	}
	pub fn end_obj(&mut self) {
		self.out.push('}');
	}
	pub fn obj_key(&mut self, name: Self, value: Self) {
		self.out.push_str(r#""${"#);
		self.extend(name);
		self.out.push_str(r#"}" = "#);
		self.extend(value);
		self.out.push_str("; ");
	}

	pub fn extend(&mut self, e: Self) {
		self.out.push_str(&e.out);
		self.used_fields.extend(e.used_fields);
	}

	#[allow(dead_code)]
	pub fn session(&self) -> NixSession {
		let mut session = None;
		for ele in &self.used_fields {
			if session.is_none() {
				session = Some(ele.session());
				continue;
			}
			let session = session.as_ref().expect("checked");
			let ele_sess = ele.session();
			assert!(
				NixSession::ptr_eq(session, &ele_sess),
				"can't mix fields from different session"
			);
		}
		session.expect("expr without fields used")
	}
	#[allow(dead_code)]
	pub fn index_attr(&mut self, s: &str) {
		let escaped = nixlike::serialize(s).expect("string");
		self.out.push('.');
		self.out.push_str(escaped.trim_end());
	}
}

#[macro_export]
macro_rules! nix_expr_inner {
	//(@munch_object FIXME: value should be arbitrary nix_expr_inner input... Time to write proc-macro?
	(@obj($o:ident) $field:ident, $($tt:tt)*) => {{
		$o.obj_key(
			NixExprBuilder::string(stringify!($field)),
			NixExprBuilder::value($field),
		);
		nix_expr_inner!(@obj($o) $($tt)*);
	}};
	(@obj($o:ident) $field:ident: $v:block, $($tt:tt)*) => {{
		$o.obj_key(
			NixExprBuilder::string(stringify!($field)),
			NixExprBuilder::serialized(&$v),
		);
		nix_expr_inner!(@obj($o) $($tt)*);
	}};
	(@obj($o:ident)) => {{}};
	(Obj { $($tt:tt)* }) => {{
		use $crate::{macros::NixExprBuilder, nix_expr_inner};
		let mut out = NixExprBuilder::object();
		nix_expr_inner!(@obj(out) $($tt)*);
		out.end_obj();
		out
	}};
	(@field($o:ident) . $var:ident $($tt:tt)*) => {{
		$o.index_attr(stringify!($var));
		nix_expr_inner!(@field($o) $($tt)*);
	}};
	(@field($o:ident) [{ $v:expr }] $($tt:tt)*) => {{
		$o.push(Index::attr(&$v));
		nix_expr_inner!(@o($o) $($tt)*);
	}};
	(@field($o:ident) [ $($var:tt)+ ] $($tt:tt)*) => {{
		$o.push(Index::Expr($crate::nix_expr_inner!($($var)+)));
		nix_expr_inner!(@o($o) $($tt)*);
	}};
	(@field($o:ident) ($($var:tt)*) $($tt:tt)*) => {
		$o.push(Index::ExprApply($crate::nix_expr_inner!($($var)+)));
		nix_expr_inner!(@o($o) $($tt)*);
	};
	(@field($o:ident)) => {};
	($field:ident $($tt:tt)*) => {{
		use $crate::{macros::NixExprBuilder, nix_expr_inner};
		// might be used if indexed
		#[allow(unused_mut)]
		let mut out = NixExprBuilder::value($field.clone());
		nix_expr_inner!(@field(out) $($tt)*);
		out
	}};
	($v:literal) => {{
		use $crate::macros::NixExprBuilder;
		NixExprBuilder::string($v)
	}};
	({$v:expr}) => {{
		use $crate::macros::NixExprBuilder;
		NixExprBuilder::serialized(&$v)
	}}
}
#[macro_export]
macro_rules! nix_expr {
	($($tt:tt)+) => {{
		use $crate::{macros::{NixExprBuilder}, Value, nix_expr_inner};
		let expr = nix_expr_inner!($($tt)+);
		Field::new(expr.session(), expr.out)
	}};
}

#[macro_export]
macro_rules! nix_go {
	(@o($o:ident) . $var:ident $($tt:tt)*) => {{
		$o.push(Index::attr(stringify!($var)));
		nix_go!(@o($o) $($tt)*);
	}};
	(@o($o:ident) [{ $v:expr }] $($tt:tt)*) => {{
		$o.push(Index::attr(&$v));
		nix_go!(@o($o) $($tt)*);
	}};
	(@o($o:ident) [ $($var:tt)+ ] $($tt:tt)*) => {{
		$o.push(Index::Expr($crate::nix_expr_inner!($($var)+)));
		nix_go!(@o($o) $($tt)*);
	}};
	(@o($o:ident) ($($var:tt)*) $($tt:tt)*) => {
		$o.push(Index::ExprApply($crate::nix_expr_inner!($($var)+)));
		nix_go!(@o($o) $($tt)*);
	};
	(@o($o:ident) | $($var:tt)*) => {
		$o.push(Index::Pipe($crate::nix_expr_inner!($($var)+)));
	};
	(@o($o:ident)) => {};
	($field:ident $($tt:tt)+) => {{
		use $crate::{nix_go, Index};
		let field = $field.clone();
		let mut out = vec![];
		nix_go!(@o(out) $($tt)*);
		field.select(out).await?
	}}
}
#[macro_export]
macro_rules! nix_go_json {
	($($tt:tt)*) => {{
		$crate::nix_go!($($tt)*).as_json().await?
	}};
}
