/*

# Collect phase

The collect phase of type check has the job of visiting all items,
determining their type, and writing that type into the `tcx.tcache`
table.  Despite its name, this table does not really operate as a
*cache*, at least not for the types of items defined within the
current crate: we assume that after the collect phase, the types of
all local items will be present in the table.

Unlike most of the types that are present in Rust, the types computed
for each item are in fact polytypes.  In "layman's terms", this means
that they are generic types that may have type parameters (more
mathematically phrased, they are universally quantified over a set of
type parameters).  Polytypes are represented by an instance of
`ty::ty_param_bounds_and_ty`.  This combines the core type along with
a list of the bounds for each parameter.  Type parameters themselves
are represented as `ty_param()` instances.

*/

import astconv::{ast_conv, ty_of_fn_decl, ty_of_arg, ast_ty_to_ty};
import rscope::*;

fn collect_item_types(ccx: @crate_ctxt, crate: @ast::crate) {

    // FIXME: hooking into the "intrinsic" root module is crude.
    // there ought to be a better approach. Attributes?

    for crate.node.module.items.each {|crate_item|
        if crate_item.ident == "intrinsic" {
            alt crate_item.node {
              ast::item_mod(m) {
                for m.items.each {|intrinsic_item|
                    alt intrinsic_item.node {
                      ast::item_iface(_, _, _) {
                        let def_id = { crate: ast::local_crate,
                                      node: intrinsic_item.id };
                        let substs = {self_r: none, self_ty: none, tps: []};
                        let ty = ty::mk_iface(ccx.tcx, def_id, substs);
                        ccx.tcx.intrinsic_ifaces.insert
                            (intrinsic_item.ident, (def_id, ty));
                      }
                      _ { }
                    }
                }
              }
              _ { }
            }
            break;
        }
    }

    visit::visit_crate(*crate, (), visit::mk_simple_visitor(@{
        visit_item: bind convert(ccx, _),
        visit_native_item: bind convert_native(ccx, _)
        with *visit::default_simple_visitor()
    }));
}

impl methods for @crate_ctxt {
    fn to_ty<RS: region_scope copy>(rs: RS, ast_ty: @ast::ty) -> ty::t {
        ast_ty_to_ty(self, rs, ast_ty)
    }
}

impl of ast_conv for @crate_ctxt {
    fn tcx() -> ty::ctxt { self.tcx }
    fn ccx() -> @crate_ctxt { self }

    fn get_item_ty(id: ast::def_id) -> ty::ty_param_bounds_and_ty {
        if id.crate != ast::local_crate {
            csearch::get_type(self.tcx, id)
        } else {
            alt self.tcx.items.find(id.node) {
              some(ast_map::node_item(item, _)) {
                ty_of_item(self, item)
              }
              some(ast_map::node_native_item(native_item, _, _)) {
                ty_of_native_item(self, native_item)
              }
              x {
                self.tcx.sess.bug(#fmt["unexpected sort of item \
                                        in get_item_ty(): %?", x]);
              }
            }
        }
    }

    fn ty_infer(span: span) -> ty::t {
        self.tcx.sess.span_bug(span,
                               "found `ty_infer` in unexpected place");
    }
}

fn get_enum_variant_types(ccx: @crate_ctxt,
                          enum_ty: ty::t,
                          variants: [ast::variant],
                          ty_params: [ast::ty_param],
                          rp: ast::region_param) {
    let tcx = ccx.tcx;

    // Create a set of parameter types shared among all the variants.
    for variants.each {|variant|
        // Nullary enum constructors get turned into constants; n-ary enum
        // constructors get turned into functions.
        let result_ty = if vec::len(variant.node.args) == 0u {
            enum_ty
        } else {
            let rs = type_rscope(rp);
            let args = variant.node.args.map { |va|
                let arg_ty = ccx.to_ty(rs, va.ty);
                {mode: ast::expl(ast::by_copy), ty: arg_ty}
            };
            ty::mk_fn(tcx, {proto: ast::proto_box,
                            inputs: args,
                            output: enum_ty,
                            ret_style: ast::return_val,
                            constraints: []})
        };
        let tpt = {bounds: ty_param_bounds(ccx, ty_params),
                   rp: rp,
                   ty: result_ty};
        tcx.tcache.insert(local_def(variant.node.id), tpt);
        write_ty_to_tcx(tcx, variant.node.id, result_ty);
    }
}

fn ensure_iface_methods(ccx: @crate_ctxt, id: ast::node_id) {
    fn store_methods<T>(ccx: @crate_ctxt, id: ast::node_id,
                        stuff: [T], f: fn@(T) -> ty::method) {
        ty::store_iface_methods(ccx.tcx, id, @vec::map(stuff, f));
    }

    let tcx = ccx.tcx;
    alt check tcx.items.get(id) {
      ast_map::node_item(@{node: ast::item_iface(_, rp, ms), _}, _) {
        store_methods::<ast::ty_method>(ccx, id, ms) {|m|
            ty_of_ty_method(ccx, m, rp)
        };
      }
      ast_map::node_item(@{node: ast::item_class(_,_,its,_,_,rp), _}, _) {
        let (_,ms) = split_class_items(its);
        // All methods need to be stored, since lookup_method
        // relies on the same method cache for self-calls
        store_methods::<@ast::method>(ccx, id, ms) {|m|
            ty_of_method(ccx, m, rp)
        };
      }
    }
}

fn compare_impl_method(tcx: ty::ctxt, sp: span, impl_m: ty::method,
                       impl_tps: uint, if_m: ty::method, substs: ty::substs,
                       self_ty: ty::t) -> ty::t {

    if impl_m.tps != if_m.tps {
        tcx.sess.span_err(sp, "method `" + if_m.ident +
                          "` has an incompatible set of type parameters");
        ty::mk_fn(tcx, impl_m.fty)
    } else if vec::len(impl_m.fty.inputs) != vec::len(if_m.fty.inputs) {
        tcx.sess.span_err(sp,#fmt["method `%s` has %u parameters \
                                   but the iface has %u",
                                  if_m.ident,
                                  vec::len(impl_m.fty.inputs),
                                  vec::len(if_m.fty.inputs)]);
        ty::mk_fn(tcx, impl_m.fty)
    } else {
        let auto_modes = vec::map2(impl_m.fty.inputs, if_m.fty.inputs, {|i, f|
            alt ty::get(f.ty).struct {
              ty::ty_param(*) | ty::ty_self
              if alt i.mode { ast::infer(_) { true } _ { false } } {
                {mode: ast::expl(ast::by_ref) with i}
              }
              _ { i }
            }
        });
        let impl_fty = ty::mk_fn(tcx, {inputs: auto_modes with impl_m.fty});

        // Add dummy substs for the parameters of the impl method
        let substs = {
            self_r: substs.self_r,
            self_ty: some(self_ty),
            tps: substs.tps + vec::from_fn(vec::len(*if_m.tps), {|i|
                ty::mk_param(tcx, i + impl_tps, {crate: 0, node: 0})
            })
        };
        let mut if_fty = ty::mk_fn(tcx, if_m.fty);
        if_fty = ty::subst(tcx, substs, if_fty);
        require_same_types(
            tcx, sp, impl_fty, if_fty,
            {|| "method `" + if_m.ident +
                 "` has an incompatible type"});
        ret impl_fty;
    }
}

fn check_methods_against_iface(ccx: @crate_ctxt,
                               tps: [ast::ty_param],
                               rp: ast::region_param,
                               selfty: ty::t,
                               a_ifacety: @ast::iface_ref,
                               ms: [@ast::method]) {

    let tcx = ccx.tcx;
    let i_bounds = ty_param_bounds(ccx, tps);
    let my_methods = convert_methods(ccx, ms, rp, i_bounds, selfty);
    let (did, tpt) = instantiate_iface_ref(ccx, a_ifacety, rp);
    if did.crate == ast::local_crate {
        ensure_iface_methods(ccx, did.node);
    }
    for vec::each(*ty::iface_methods(tcx, did)) {|if_m|
        alt vec::find(my_methods, {|m| if_m.ident == m.mty.ident}) {
          some({mty: m, id, span}) {
            if m.purity != if_m.purity {
                ccx.tcx.sess.span_err(
                    span, #fmt["method `%s`'s purity \
                                not match the iface method's \
                                purity", m.ident]);
            }
            let mt = compare_impl_method(
                ccx.tcx, span, m, vec::len(tps),
                if_m, tpt.substs, selfty);
            let old = tcx.tcache.get(local_def(id));
            if old.ty != mt {
                tcx.tcache.insert(
                    local_def(id),
                    {bounds: old.bounds,
                     rp: old.rp,
                     ty: mt});
                write_ty_to_tcx(tcx, id, mt);
            }
          }
          none {
            tcx.sess.span_err(
                a_ifacety.path.span,
                #fmt["missing method `%s`", if_m.ident]);
          }
        } // alt
    } // |if_m|
} // fn

fn convert_class_item(ccx: @crate_ctxt,
                      rp: ast::region_param,
                      bounds: @[ty::param_bounds],
                      v: ast_util::ivar) {
    /* we want to do something here, b/c within the
    scope of the class, it's ok to refer to fields &
    methods unqualified */
    /* they have these types *within the scope* of the
    class. outside the class, it's done with expr_field */
    let tt = ccx.to_ty(type_rscope(rp), v.ty);
    write_ty_to_tcx(ccx.tcx, v.id, tt);
    /* add the field to the tcache */
    ccx.tcx.tcache.insert(local_def(v.id), {bounds: bounds, rp: rp, ty: tt});
}

fn convert_methods(ccx: @crate_ctxt,
                   ms: [@ast::method],
                   rp: ast::region_param,
                   i_bounds: @[ty::param_bounds],
                   self_ty: ty::t)
    -> [{mty: ty::method, id: ast::node_id, span: span}] {

    let tcx = ccx.tcx;
    vec::map(ms) { |m|
        write_ty_to_tcx(tcx, m.self_id, self_ty);
        let bounds = ty_param_bounds(ccx, m.tps);
        let mty = ty_of_method(ccx, m, rp);
        let fty = ty::mk_fn(tcx, mty.fty);
        tcx.tcache.insert(
            local_def(m.id),
            // n.b. This code is kind of sketchy (concat'ing i_bounds
            // with bounds), but removing *i_bounds breaks other stuff
            {bounds: @(*i_bounds + *bounds), rp: rp, ty: fty});
        write_ty_to_tcx(tcx, m.id, fty);
        {mty: mty, id: m.id, span: m.span}
    }
}

fn convert(ccx: @crate_ctxt, it: @ast::item) {
    let tcx = ccx.tcx;
    alt it.node {
      // These don't define types.
      ast::item_mod(_) {}
      ast::item_native_mod(m) {
        if syntax::attr::native_abi(it.attrs) ==
            either::right(ast::native_abi_rust_intrinsic) {
            for m.items.each { |item| check_intrinsic_type(ccx, item); }
        }
      }
      ast::item_enum(variants, ty_params, rp) {
        let tpt = ty_of_item(ccx, it);
        write_ty_to_tcx(tcx, it.id, tpt.ty);
        get_enum_variant_types(ccx, tpt.ty, variants,
                               ty_params, rp);
      }
      ast::item_impl(tps, rp, ifce, selfty, ms) {
        let i_bounds = ty_param_bounds(ccx, tps);
        let selfty = ccx.to_ty(type_rscope(rp), selfty);
        write_ty_to_tcx(tcx, it.id, selfty);
        tcx.tcache.insert(local_def(it.id),
                          {bounds: i_bounds,
                           rp: rp,
                           ty: selfty});
        alt ifce {
          some(t) {
            check_methods_against_iface(
                ccx, tps, rp,
                selfty, t, ms);
          }
          _ {
            // Still have to do this to write method types
            // into the table
            convert_methods(
                ccx, ms, rp,
                i_bounds, selfty);
          }
        }
      }
      ast::item_res(decl, tps, _, dtor_id, ctor_id, rp) {
        let {bounds, substs} = mk_substs(ccx, tps, rp);
        let def_id = local_def(it.id);
        let t_arg = ty_of_arg(ccx, type_rscope(rp),
                                       decl.inputs[0], none);
        let t_res = ty::mk_res(tcx, def_id, t_arg.ty, substs);

        let t_ctor = ty::mk_fn(tcx, {
            proto: ast::proto_box,
            inputs: [{mode: ast::expl(ast::by_copy), ty: t_arg.ty}],
            output: t_res,
            ret_style: ast::return_val, constraints: []
        });
        let t_dtor = ty::mk_fn(tcx, {
            proto: ast::proto_box,
            inputs: [t_arg], output: ty::mk_nil(tcx),
            ret_style: ast::return_val, constraints: []
        });
        write_ty_to_tcx(tcx, it.id, t_res);
        write_ty_to_tcx(tcx, ctor_id, t_ctor);
        tcx.tcache.insert(local_def(ctor_id),
                          {bounds: bounds,
                           rp: rp,
                           ty: t_ctor});
        tcx.tcache.insert(def_id, {bounds: bounds,
                                   rp: rp,
                                   ty: t_res});
        write_ty_to_tcx(tcx, dtor_id, t_dtor);
      }
      ast::item_iface(*) {
        let tpt = ty_of_item(ccx, it);
        #debug["item_iface(it.id=%d, tpt.ty=%s)",
               it.id, ty_to_str(tcx, tpt.ty)];
        write_ty_to_tcx(tcx, it.id, tpt.ty);
        ensure_iface_methods(ccx, it.id);
      }
      ast::item_class(tps, ifaces, members, ctor, m_dtor, rp) {
        // Write the class type
        let tpt = ty_of_item(ccx, it);
        write_ty_to_tcx(tcx, it.id, tpt.ty);
        tcx.tcache.insert(local_def(it.id), {bounds: tpt.bounds,
              rp: rp, ty: tpt.ty});
        // Write the ctor type
        let t_ctor =
            ty::mk_fn(
                tcx,
                ty_of_fn_decl(ccx,
                                       empty_rscope,
                                       ast::proto_any,
                                       ctor.node.dec,
                                       none));
        write_ty_to_tcx(tcx, ctor.node.id, t_ctor);
        tcx.tcache.insert(local_def(ctor.node.id),
                          {bounds: tpt.bounds,
                           rp: ast::rp_none,
                           ty: t_ctor});
        option::iter(m_dtor) {|dtor|
            // Write the dtor type
            let t_dtor = ty::mk_fn(
                tcx,
                // not sure about empty_rscope
                // FIXME
                ty_of_fn_decl(ccx,
                                       empty_rscope,
                                       ast::proto_any,
                                       ast_util::dtor_dec(),
                                       none));
            write_ty_to_tcx(tcx, dtor.node.id, t_dtor);
            tcx.tcache.insert(local_def(dtor.node.id),
                              {bounds: tpt.bounds,
                               rp: ast::rp_none,
                               ty: t_dtor});
        };
        ensure_iface_methods(ccx, it.id);
        /* FIXME: check for proper public/privateness */
        // Write the type of each of the members
        let (fields, methods) = split_class_items(members);
        for fields.each {|f|
           convert_class_item(ccx, rp, tpt.bounds, f);
        }
        // The selfty is just the class type
        let {bounds:_, substs} = mk_substs(ccx, tps, rp);
        let selfty = ty::mk_class(tcx, local_def(it.id), substs);
        // Need to convert all methods so we can check internal
        // references to private methods

        // NDM to TJC---I think we ought to be using bounds here, not @[].
        // But doing so causes errors later on.
        convert_methods(ccx, methods, rp, @[], selfty);

        /*
        Finally, check that the class really implements the ifaces
        that it claims to implement.
        */
        for ifaces.each { |ifce|
            check_methods_against_iface(ccx, tps, rp, selfty,
                                        ifce, methods);
            let t = ty::node_id_to_type(tcx, ifce.id);

            // FIXME: This assumes classes only implement
            // non-parameterized ifaces. add a test case for
            // a class implementing a parameterized iface.
            // -- tjc (#1726)
            tcx.tcache.insert(local_def(ifce.id), no_params(t));
        }
      }
      _ {
        // This call populates the type cache with the converted type
        // of the item in passing. All we have to do here is to write
        // it into the node type table.
        let tpt = ty_of_item(ccx, it);
        write_ty_to_tcx(tcx, it.id, tpt.ty);
      }
    }
}
fn convert_native(ccx: @crate_ctxt, i: @ast::native_item) {
    // As above, this call populates the type table with the converted
    // type of the native item. We simply write it into the node type
    // table.
    let tpt = ty_of_native_item(ccx, i);
    alt i.node {
      ast::native_item_fn(_, _) {
        write_ty_to_tcx(ccx.tcx, i.id, tpt.ty);
      }
    }
}

fn ty_of_method(ccx: @crate_ctxt,
                m: @ast::method,
                rp: ast::region_param) -> ty::method {
    {ident: m.ident,
     tps: ty_param_bounds(ccx, m.tps),
     fty: ty_of_fn_decl(ccx, type_rscope(rp), ast::proto_bare,
                                 m.decl, none),
     purity: m.decl.purity,
     vis: m.vis}
}

fn ty_of_ty_method(self: @crate_ctxt,
                   m: ast::ty_method,
                   rp: ast::region_param) -> ty::method {
    {ident: m.ident,
     tps: ty_param_bounds(self, m.tps),
     fty: ty_of_fn_decl(self, type_rscope(rp), ast::proto_bare,
                                 m.decl, none),
     // assume public, because this is only invoked on iface methods
     purity: m.decl.purity, vis: ast::public}
}

fn check_intrinsic_type(ccx: @crate_ctxt, it: @ast::native_item) {
    fn param(ccx: @crate_ctxt, n: uint) -> ty::t {
        ty::mk_param(ccx.tcx, n, local_def(0))
    }
    fn arg(m: ast::rmode, ty: ty::t) -> ty::arg {
        {mode: ast::expl(m), ty: ty}
    }
    let tcx = ccx.tcx;
    let (n_tps, inputs, output) = alt it.ident {
      "size_of" |
      "pref_align_of" | "min_align_of" { (1u, [], ty::mk_uint(ccx.tcx)) }
      "get_tydesc" { (1u, [], ty::mk_nil_ptr(tcx)) }
      "init" { (1u, [], param(ccx, 0u)) }
      "forget" { (1u, [arg(ast::by_move, param(ccx, 0u))],
                  ty::mk_nil(tcx)) }
      "reinterpret_cast" { (2u, [arg(ast::by_ref, param(ccx, 0u))],
                            param(ccx, 1u)) }
      "addr_of" { (1u, [arg(ast::by_ref, param(ccx, 0u))],
                   ty::mk_imm_ptr(tcx, param(ccx, 0u))) }
      "needs_drop" { (1u, [], ty::mk_bool(tcx)) }

      "visit_ty" {
        assert ccx.tcx.intrinsic_ifaces.contains_key("ty_visitor");
        let (_, visitor_iface) = ccx.tcx.intrinsic_ifaces.get("ty_visitor");
        (1u, [arg(ast::by_ref, visitor_iface)], ty::mk_nil(tcx))
      }

      other {
        tcx.sess.span_err(it.span, "unrecognized intrinsic function: `" +
                          other + "`");
        ret;
      }
    };
    let fty = ty::mk_fn(tcx, {proto: ast::proto_bare,
                              inputs: inputs, output: output,
                              ret_style: ast::return_val,
                              constraints: []});
    let i_ty = ty_of_native_item(ccx, it);
    let i_n_tps = (*i_ty.bounds).len();
    if i_n_tps != n_tps {
        tcx.sess.span_err(it.span, #fmt("intrinsic has wrong number \
                                         of type parameters. found %u, \
                                         expected %u", i_n_tps, n_tps));
    } else {
        require_same_types(
            tcx, it.span, i_ty.ty, fty,
            {|| #fmt["intrinsic has wrong type. \
                      expected %s",
                     ty_to_str(ccx.tcx, fty)]});
    }
}


/*
  Instantiates the path for the given iface reference, assuming that
  it's bound to a valid iface type. Returns the def_id for the defining
  iface. Fails if the type is a type other than an iface type.
 */
fn instantiate_iface_ref(ccx: @crate_ctxt, t: @ast::iface_ref,
                         rp: ast::region_param)
    -> (ast::def_id, ty_param_substs_and_ty) {

    let sp = t.path.span, err = "can only implement interface types",
        sess = ccx.tcx.sess;

    let rscope = type_rscope(rp);

    alt lookup_def_tcx(ccx.tcx, t.path.span, t.id) {
      ast::def_ty(t_id) {
        let tpt = astconv::ast_path_to_ty(ccx, rscope, t_id, t.path, t.id);
        alt ty::get(tpt.ty).struct {
           ty::ty_iface(*) {
              (t_id, tpt)
           }
           _ { sess.span_fatal(sp, err); }
        }
      }
      _ {
          sess.span_fatal(sp, err);
      }
    }
}

fn ty_of_item(ccx: @crate_ctxt, it: @ast::item)
    -> ty::ty_param_bounds_and_ty {

    let def_id = local_def(it.id);
    let tcx = ccx.tcx;
    alt tcx.tcache.find(def_id) {
      some(tpt) { ret tpt; }
      _ {}
    }
    alt it.node {
      ast::item_const(t, _) {
        let typ = ccx.to_ty(empty_rscope, t);
        let tpt = no_params(typ);
        tcx.tcache.insert(local_def(it.id), tpt);
        ret tpt;
      }
      ast::item_fn(decl, tps, _) {
        let bounds = ty_param_bounds(ccx, tps);
        let tofd = ty_of_fn_decl(ccx, empty_rscope, ast::proto_bare,
                                          decl, none);
        let tpt = {bounds: bounds,
                   rp: ast::rp_none, // functions do not have a self
                   ty: ty::mk_fn(ccx.tcx, tofd)};
        ccx.tcx.tcache.insert(local_def(it.id), tpt);
        ret tpt;
      }
      ast::item_ty(t, tps, rp) {
        alt tcx.tcache.find(local_def(it.id)) {
          some(tpt) { ret tpt; }
          none { }
        }

        let tpt = {
            let ty = {
                let t0 = ccx.to_ty(type_rscope(rp), t);
                // Do not associate a def id with a named, parameterized type
                // like "foo<X>".  This is because otherwise ty_to_str will
                // print the name as merely "foo", as it has no way to
                // reconstruct the value of X.
                if !vec::is_empty(tps) { t0 } else {
                    ty::mk_with_id(tcx, t0, def_id)
                }
            };
            {bounds: ty_param_bounds(ccx, tps), rp: rp, ty: ty}
        };

        tcx.tcache.insert(local_def(it.id), tpt);
        ret tpt;
      }
      ast::item_res(decl, tps, _, _, _, rp) {
        let {bounds, substs} = mk_substs(ccx, tps, rp);
        let t_arg = ty_of_arg(ccx, type_rscope(rp),
                                       decl.inputs[0], none);
        let t = ty::mk_res(tcx, local_def(it.id), t_arg.ty, substs);
        let t_res = {bounds: bounds, rp: rp, ty: t};
        tcx.tcache.insert(local_def(it.id), t_res);
        ret t_res;
      }
      ast::item_enum(_, tps, rp) {
        // Create a new generic polytype.
        let {bounds, substs} = mk_substs(ccx, tps, rp);
        let t = ty::mk_enum(tcx, local_def(it.id), substs);
        let tpt = {bounds: bounds, rp: rp, ty: t};
        tcx.tcache.insert(local_def(it.id), tpt);
        ret tpt;
      }
      ast::item_iface(tps, rp, ms) {
        let {bounds, substs} = mk_substs(ccx, tps, rp);
        let t = ty::mk_iface(tcx, local_def(it.id), substs);
        let tpt = {bounds: bounds, rp: rp, ty: t};
        tcx.tcache.insert(local_def(it.id), tpt);
        ret tpt;
      }
      ast::item_class(tps, _, _, _, _, rp) {
          let {bounds,substs} = mk_substs(ccx, tps, rp);
          let t = ty::mk_class(tcx, local_def(it.id), substs);
          let tpt = {bounds: bounds, rp: rp, ty: t};
          tcx.tcache.insert(local_def(it.id), tpt);
          ret tpt;
      }
      ast::item_impl(*) | ast::item_mod(_) |
      ast::item_native_mod(_) { fail; }
    }
}

fn ty_of_native_item(ccx: @crate_ctxt, it: @ast::native_item)
    -> ty::ty_param_bounds_and_ty {
    alt it.node {
      ast::native_item_fn(fn_decl, params) {
        ret ty_of_native_fn_decl(ccx, fn_decl, params,
                                 local_def(it.id));
      }
    }
}
fn ty_param_bounds(ccx: @crate_ctxt,
                   params: [ast::ty_param]) -> @[ty::param_bounds] {

    fn compute_bounds(ccx: @crate_ctxt,
                      param: ast::ty_param) -> ty::param_bounds {
        @vec::flat_map(*param.bounds) { |b|
            alt b {
              ast::bound_send { [ty::bound_send] }
              ast::bound_copy { [ty::bound_copy] }
              ast::bound_iface(t) {
                let ity = ast_ty_to_ty(ccx, empty_rscope, t);
                alt ty::get(ity).struct {
                  ty::ty_iface(*) {
                    [ty::bound_iface(ity)]
                  }
                  _ {
                    ccx.tcx.sess.span_err(
                        t.span, "type parameter bounds must be \
                                 interface types");
                    []
                  }
                }
              }
            }
        }
    }

    @params.map { |param|
        alt ccx.tcx.ty_param_bounds.find(param.id) {
          some(bs) { bs }
          none {
            let bounds = compute_bounds(ccx, param);
            ccx.tcx.ty_param_bounds.insert(param.id, bounds);
            bounds
          }
        }
    }
}

fn ty_of_native_fn_decl(ccx: @crate_ctxt,
                        decl: ast::fn_decl,
                        ty_params: [ast::ty_param],
                        def_id: ast::def_id) -> ty::ty_param_bounds_and_ty {

    let bounds = ty_param_bounds(ccx, ty_params);
    let rb = in_binding_rscope(empty_rscope);
    let input_tys = decl.inputs.map { |a| ty_of_arg(ccx, rb, a, none) };
    let output_ty = ast_ty_to_ty(ccx, rb, decl.output);

    let t_fn = ty::mk_fn(ccx.tcx, {proto: ast::proto_bare,
                                   inputs: input_tys,
                                   output: output_ty,
                                   ret_style: ast::return_val,
                                   constraints: []});
    let tpt = {bounds: bounds, rp: ast::rp_none, ty: t_fn};
    ccx.tcx.tcache.insert(def_id, tpt);
    ret tpt;
}

fn mk_ty_params(ccx: @crate_ctxt, atps: [ast::ty_param])
    -> {bounds: @[ty::param_bounds], params: [ty::t]} {

    let mut i = 0u;
    let bounds = ty_param_bounds(ccx, atps);
    {bounds: bounds,
     params: vec::map(atps, {|atp|
         let t = ty::mk_param(ccx.tcx, i, local_def(atp.id));
         i += 1u;
         t
     })}
}

fn mk_substs(ccx: @crate_ctxt, atps: [ast::ty_param], rp: ast::region_param)
    -> {bounds: @[ty::param_bounds], substs: ty::substs} {

    let {bounds, params} = mk_ty_params(ccx, atps);
    let self_r = alt rp {
      ast::rp_self { some(ty::re_bound(ty::br_self)) }
      ast::rp_none { none }
    };
    {bounds: bounds, substs: {self_r: self_r, self_ty: none, tps: params}}
}
