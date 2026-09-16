use cordis::Context;

fn main() {
    let ctx = Context::new();
    let _ = cordis::__internal::generation_cleanup_admitted(&ctx);
}
