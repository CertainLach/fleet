//! Serialization/deserialization for nix subset usable for static configurations
//! Serialized results from this library are readable by both this library and standard nix tools

use linked_hash_map::LinkedHashMap;
use peg::str::LineCol;
use se_impl::MySerialize;
use serde::{Deserialize, Serialize};

mod de_impl;
mod se_impl;
mod to_string;

pub use to_string::escape_string;

#[derive(thiserror::Error, Debug)]
pub enum Error {
	#[error("bad number")]
	BadNumber,
	#[error("expected {0}")]
	Expected(&'static str),
	#[error("parse error")]
	ParseError(#[from] peg::error::ParseError<LineCol>),
	#[error("{0}")]
	Custom(String),
	#[error("io: {0}")]
	Io(#[from] std::io::Error),
	#[error("fmt: {0}")]
	Fmt(#[from] std::fmt::Error),
}

#[derive(Debug)]
pub enum Value {
	Number(i64),
	String(String),
	Boolean(bool),
	Object(LinkedHashMap<String, Value>),
	Array(Vec<Value>),
	Null,
}

peg::parser! {
pub grammar nixlike() for str {
	rule number() -> i64
		= quiet! { v:$(['0'..='9' | '+' | '-']+) {? v.parse().map_err(|_| "<number>")} } / expected!("<number>")
	rule string_char() -> &'input str
		= "\\\"" { "\"" }
		/ "\\\\" { "\\" }
		/ "\\n" { "\n" }
		/ "\\t" { "\t" }
		/ "\\r" { "\r" }
		/ "\\$" { "$" }
		/ c:$([_]) { c }
	rule string() -> String
		= quiet! { "\"" v:(!"\"" c:string_char() {c})* "\"" { v.into_iter().collect() } } / expected!("<string>")
	rule boolean() -> bool
		= quiet! { "true" {true}
		/ "false" {false} } / expected!("<boolean>")
	rule indent() -> String
		= quiet! {
			s:$(['a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-']+) { s.to_owned() }
			/ "\"" s:$(['a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' | '.']+) "\"" { s.to_owned() }
		} / expected!("<identifier>")
	rule object() -> LinkedHashMap<String, Value>
		= "{" _
			e:(k:indent()++(_ "." _) _ "=" _ v:value() _ ";" _ {(k, v)})*
		"}" {?
			let mut out = LinkedHashMap::new();
			for (k, v) in e {
				let mut map = &mut out;
				for v in k.iter().take(k.len() - 1) {
					map = match map.entry(v.clone()).or_insert_with(|| Value::Object(Default::default())) {
						Value::Object(v) => v,
						_ => return Err("expected object"),
					}
				}

				let key = k.into_iter().last().unwrap();
				if map.contains_key(&key) {
					return Err("can't override object");
				}
				map.insert(key, v);
			}
			Ok(out)
		}

	rule array() -> Vec<Value>
		= "[" _ v:value()**_ _ "]" {v}

	rule value() -> Value
		= o:object() { Value::Object(o) }
		/ a:array() { Value::Array(a) }
		/ s:string() { Value::String(s) }
		/ "null" { Value::Null }
		/ b:boolean() { Value::Boolean(b) }
		/ n:number() { Value::Number(n) }

	pub rule root() -> Value
		= _ v:value() _ { v }

	rule _()
		= ( quiet!{ [' ' | '\t' | '\n']+ }
		/ "#" (!['\n'] [_])* "\n" )*
}
}

pub fn parse_str<'de, D: Deserialize<'de>>(s: &str) -> Result<D, Error> {
	let value = nixlike::root(s)?;
	D::deserialize(value)
}

pub fn parse_value<'de, D: Deserialize<'de>>(value: Value) -> Result<D, Error> {
	D::deserialize(value)
}

pub fn serialize_value_pretty(value: Value) -> String {
	to_string::write_nix(&value)
}

pub fn serialize<S: Serialize>(value: S) -> Result<String, Error> {
	let value: Value = value.serialize(MySerialize)?;
	Ok(serialize_value_pretty(value))
}

pub fn format_identifier(i: &str) -> String {
	let mut out = String::new();
	to_string::write_identifier(i, &mut out);
	out
}

#[test]
fn test() {
	assert_eq!(serialize("Hello\nworld").unwrap(), "\"Hello\\nworld\"\n");
}
pub fn format_nix(value: &String) -> String {
	let (_, out) = alejandra::format::in_memory("".to_owned(), value.to_owned());
	out
}
