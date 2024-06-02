// use bindgen::callbacks::ParseCallbacks;
// use std::path::PathBuf;
//
// #[derive(Debug)]
// struct StripPrefix;
// impl ParseCallbacks for StripPrefix {
//     fn item_name(&self, name: &str) -> Option<String> {
//         name.strip_prefix("nix_").map(ToOwned::to_owned)
//     }
// }


fn main() {
	//
	// let mut libnix = bindgen::builder().header_contents("nix.h", "
	// 	#define GC_THREADS
	// 	#include <gc/gc.h>
	// 	#include <nix_api_expr.h>
	// 	#include <nix_api_store.h>
	// 	#include <nix_api_util.h>
	// 	#include <nix_api_value.h>
	// ").parse_callbacks(Box::new(StripPrefix));
	//
	// for header in pkg_config::probe_library("nix-expr-c").expect("nix-expr-c").include_paths.into_iter().chain(pkg_config::probe_library("bdw-gc").expect("bdw-gc").include_paths.into_iter()) {
	// 	libnix = libnix.clang_arg(format!("-I{}", header.to_str().expect("path is utf-8")));
	// }
	//
	// let mut out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo"));
	// out.push("bindings.rs");
	// libnix.generate().expect("generate bindings").write_to_file(out).expect("write bindings");
}
