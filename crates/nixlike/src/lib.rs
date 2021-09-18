use std::collections::BTreeMap;

use peg::str::LineCol;
use se_impl::MySerialize;
use serde::{Deserialize, Serialize};

mod de_impl;
mod se_impl;
mod to_string;

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
	Object(BTreeMap<String, Value>),
	Array(Vec<Value>),
	Null,
}

peg::parser! {
pub grammar nixlike() for str {
	rule number() -> i64
		= v:$(['0'..='9' | '+' | '-']+) {? v.parse().map_err(|_| "<number>")}
	rule string() -> String
		= "\"" v:$((!"\"" [_])+) "\"" { v.to_owned() }
	rule boolean() -> bool
		= "true" {true}
		/ "false" {false}
	rule indent() -> String
		= s:$(['a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-']+) { s.to_owned() }
	rule object() -> BTreeMap<String, Value>
		= "{" _
			e:(k:indent()++(_ "." _) _ "=" _ v:value() _ ";" _ {(k, v)})*
		"}" {?
			let mut out = BTreeMap::new();
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

pub fn serialize_value_pretty(value: Value) -> Result<String, Error> {
	to_string::write_nix(&value)
}

pub fn serialize<S: Serialize>(value: S) -> Result<String, Error> {
	let value: Value = value.serialize(MySerialize)?;
	serialize_value_pretty(value)
}

#[test]
fn test() {
	let v: serde_json::Value = parse_str(
		r#"
			{
				b.c = 2;
				b.d = "hello";
				c = {
					k = 123;
					p = 231;
					ll = [1 2 3 [] [[4 5 6]] ];
				};
			}
		"#,
	)
	.unwrap();
	let s: String = serialize(v).unwrap();
	println!("{}", s);
}
