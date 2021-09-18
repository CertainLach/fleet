use crate::{Error, Value};

fn write_nix_obj_key_buf(
	k: &str,
	v: &Value,
	out: &mut String,
	indent: &mut String,
) -> Result<(), Error> {
	use std::fmt::Write;
	write!(out, "{}", k)?;
	match v {
		Value::Object(o) if o.len() == 1 => {
			let (k, v) = o.iter().next().unwrap();
			write!(out, ".")?;
			write_nix_obj_key_buf(k, v, out, indent)?;
		}
		v => {
			write!(out, " = ")?;
			write_nix_buf(v, out, indent)?;
			writeln!(out, ";")?;
		}
	}
	Ok(())
}

fn write_nix_buf(value: &Value, out: &mut String, indent: &mut String) -> Result<(), Error> {
	use std::fmt::Write;
	match value {
		Value::Null => write!(out, "null")?,
		Value::Boolean(v) => write!(out, "{:?}", v)?,
		Value::Number(n) => write!(out, "{}", n)?,
		Value::String(s) => write!(out, "{:?}", s)?,
		Value::Array(a) => {
			if a.is_empty() {
				write!(out, "[ ]")?;
			} else {
				writeln!(out, "[")?;
				let old_len = indent.len();
				indent.push_str("  ");
				for item in a {
					write!(out, "{}", indent)?;
					write_nix_buf(item, out, indent)?;
					writeln!(out)?;
				}
				indent.truncate(old_len);
				write!(out, "{}]", indent)?;
			}
		}
		Value::Object(obj) => {
			if obj.is_empty() {
				write!(out, "{{ }}")?;
			} else {
				writeln!(out, "{{")?;
				let old_len = indent.len();
				indent.push_str("  ");
				for (k, v) in obj {
					write!(out, "{}", indent)?;
					write_nix_obj_key_buf(k, v, out, indent)?;
				}
				indent.truncate(old_len);
				write!(out, "{}}}", indent)?;
			}
		}
	};
	Ok(())
}

pub fn write_nix(value: &Value) -> Result<String, Error> {
	let mut out = String::new();
	let mut indent = String::new();

	write_nix_buf(value, &mut out, &mut indent)?;
	Ok(out)
}
