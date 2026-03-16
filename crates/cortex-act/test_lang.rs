extern "C" {
    fn tree_sitter_json() -> *const ();
}
fn main() {
    let lang: tree_sitter::Language = unsafe { tree_sitter::Language::new(tree_sitter_json() as *const _) };
}
