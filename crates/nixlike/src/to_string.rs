use crate::Value;
use dprint_core::formatting::{
	condition_resolvers, conditions, format, ConditionResolverContext, Info, PrintItems,
	PrintOptions, Signal,
};

fn write_nix_obj_key_buf(k: &str, v: &Value, out: &mut PrintItems) {
	if k.contains(".") {
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

fn write_nix_buf(value: &Value, out: &mut PrintItems) {
	match value {
		Value::Null => out.push_str("null"),
		Value::Boolean(v) => out.push_str(if *v { "true" } else { "false" }),
		Value::Number(n) => out.push_str(&format!("{}", n)),
		Value::String(s) => out.push_str(&format!(
			"\"{}\"",
			s.replace('\\', "\\\\")
				.replace('"', "\\\"")
				.replace('\n', "\\n")
				.replace('\t', "\\t")
				.replace('\r', "\\r")
				.replace("$", "\\$")
		)),
		Value::Array(a) => {
			if a.is_empty() {
				out.push_str("[ ]");
			} else {
				let start_info = Info::new("start");
				let end_info = Info::new("end");
				let is_multiple_lines = move |ctx: &mut ConditionResolverContext| {
					condition_resolvers::is_multiple_lines(ctx, &start_info, &end_info)
				};
				out.push_str("[");
				out.push_info(start_info);
				out.push_signal(Signal::StartIndent);
				out.push_condition(conditions::if_true_or(
					"array start",
					is_multiple_lines,
					Signal::NewLine.into(),
					Signal::SpaceOrNewLine.into(),
				));
				for item in a {
					write_nix_buf(item, out);
					out.push_condition(conditions::if_true_or(
						"element separator",
						is_multiple_lines,
						Signal::NewLine.into(),
						Signal::SpaceOrNewLine.into(),
					));
				}
				out.push_signal(Signal::FinishIndent);
				out.push_info(end_info);
				out.push_str("]");
			}
		}
		Value::Object(obj) => {
			if obj.is_empty() {
				out.push_str("{ }")
			} else {
				let start_info = Info::new("start");
				let end_info = Info::new("end");
				let is_multiple_lines = move |ctx: &mut ConditionResolverContext| {
					condition_resolvers::is_multiple_lines(ctx, &start_info, &end_info)
				};
				out.push_str("{");
				out.push_info(start_info);
				out.push_signal(Signal::StartIndent);
				out.push_condition(conditions::if_true_or(
					"object start",
					is_multiple_lines,
					Signal::NewLine.into(),
					Signal::SpaceOrNewLine.into(),
				));
				for (k, v) in obj {
					write_nix_obj_key_buf(k, v, out);
					out.push_condition(conditions::if_true_or(
						"element separator",
						is_multiple_lines,
						Signal::NewLine.into(),
						Signal::SpaceOrNewLine.into(),
					));
				}
				out.push_signal(Signal::FinishIndent);
				out.push_info(end_info);
				out.push_str("}");
			}
		}
	};
}

pub fn write_nix(value: &Value) -> String {
	format(
		|| {
			let mut items = PrintItems::new();
			write_nix_buf(value, &mut items);
			items
		},
		PrintOptions {
			max_width: 120,
			use_tabs: false,
			indent_width: 2,
			new_line_text: "\n",
		},
	)
}
