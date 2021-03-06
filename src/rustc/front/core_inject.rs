import driver::session::session;
import syntax::codemap;
import syntax::ast;
import syntax::ast_util::*;
import syntax::attr;

export maybe_inject_libcore_ref;

fn maybe_inject_libcore_ref(sess: session,
                            crate: @ast::crate) -> @ast::crate {
    if use_core(crate) {
        inject_libcore_ref(sess, crate)
    } else {
        crate
    }
}

fn use_core(crate: @ast::crate) -> bool {
    !attr::attrs_contains_name(crate.node.attrs, "no_core")
}

fn inject_libcore_ref(sess: session,
                      crate: @ast::crate) -> @ast::crate {

    fn spanned<T: copy>(x: T) -> @ast::spanned<T> {
        ret @{node: x,
            span: dummy_sp()};
    }

    let n1 = sess.next_node_id();
    let n2 = sess.next_node_id();

    let vi1 = @{node: ast::view_item_use("core", [], n1),
                attrs: [],
                vis: ast::public,
                span: dummy_sp()};
    let vp = spanned(ast::view_path_glob(ident_to_path(dummy_sp(), "core"),
                                         n2));
    let vi2 = @{node: ast::view_item_import([vp]),
                attrs: [],
                vis: ast::public,
                span: dummy_sp()};

    let vis = [vi1, vi2] + crate.node.module.view_items;

    ret @{node: {module: { view_items: vis with crate.node.module }
                 with crate.node} with *crate }
}
