use crate::Value;

fn write_nix_obj_key_buf(k: &str, v: &Value, out: &mut String) {
	if k.contains('.') {
		out.push_str("\"");
		out.push_str(k);
		out.push_str("\"");
	} else {
		out.push_str(k);
	}
	match v {
		Value::Object(o) if o.len() == 1 => {
			let (k, v) = o.iter().next().unwrap();

			out.push_str(".");
			write_nix_obj_key_buf(k, v, out);
		}
		v => {
			out.push_str(" = ");
			write_nix_buf(v, out);
			out.push_str(";");
		}
	}
}

fn write_nix_str(str: &str, out: &mut String) {
	out.push_str(&format!(
		"\"{}\"",
		str.replace('\\', "\\\\")
			.replace('"', "\\\"")
			.replace('\n', "\\n")
			.replace('\t', "\\t")
			.replace('\r', "\\r")
			.replace('$', "\\$")
	))
}

fn write_nix_buf(value: &Value, out: &mut String) {
	match value {
		Value::Null => out.push_str("null"),
		Value::Boolean(v) => out.push_str(if *v { "true" } else { "false" }),
		Value::Number(n) => out.push_str(&format!("{}", n)),
		Value::String(s) => write_nix_str(s, out),
		Value::Array(a) => {
			if a.is_empty() {
				out.push_str("[ ]");
			} else {
				out.push('[');
				for item in a {
					write_nix_buf(item, out);
					out.push('\n');
				}
				out.push(']');
			}
		}
		Value::Object(obj) => {
			if obj.is_empty() {
				out.push_str("{ }")
			} else {
				out.push('{');
				for (k, v) in obj {
					write_nix_obj_key_buf(k, v, out);
					out.push('\n');
				}
				out.push('}');
			}
		}
	};
}

pub fn write_nix(value: &Value) -> String {
	let mut out = String::new();
	write_nix_buf(value, &mut out);
	let (_, out) = alejandra::format::in_memory("".to_owned(), out);
	out
}
