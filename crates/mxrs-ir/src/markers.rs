//! Phase 4 marker-type contract: the trait surface `mxrs-typegen`-generated
//! code implements, and the newtype `mxrs-dsl::EntityBuilder::association`
//! now *requires* in place of a raw string target. Kept here rather than in
//! `mxrs-typegen` itself so a user's generated-markers module only needs to
//! depend on `mxrs-ir` (already a transitive dependency via `mxrs-dsl`),
//! not on the codegen tool — the same "no capability exists only through
//! generated code" split `mxrs-macros` (Phase 8) is required to follow for
//! the same reason.
//!
//! **`mxrs-dsl` wiring** (`EntityBuilder::association`'s target parameter
//! is `Ref<M>`, not `impl Into<String>`): a typo'd or renamed association
//! target — even a same-module one — now fails `cargo build` via a
//! "no such marker type" error, not `mxrs-writer`'s runtime
//! `UnknownAssociationTarget`. The runtime check still exists and still
//! matters: a `Ref<M>` only proves the *marker type* exists (i.e. some
//! manifest once declared that name), not that the specific `ProjectDecl`
//! being written actually includes that entity — those are two different
//! guarantees, and only the second one is enforceable at compile time
//! without also making the whole `ProjectBuilder` invocation type-level
//! (out of scope for this pass). See
//! `mxrs-writer::domain::resolve_association`'s doc comment for the
//! consequence this has on local-vs-cross-module routing: a same-module
//! `Ref<M>` still resolves to a fully-qualified `"Module.Entity"` string
//! (there's no "short form" anymore), so routing can no longer key off
//! "does the target string contain a dot."
//!
//! `AssociationDecl.target` itself is still a plain `String`, deliberately:
//! only the *builder's* entry point requires a marker; the underlying IR
//! stays constructible directly (with a hand-written string target,
//! unqualified or qualified) for lower-level callers — `mxrs-writer`'s own
//! test suite does this to exercise `synchronize_domain_associations`
//! without going through `mxrs-dsl` at all.

use std::marker::PhantomData;

/// Implemented by a generated marker type for one Mendix entity (e.g.
/// `markers::Sales::Order`, generated from a manifest's `Sales.Order`
/// declaration). Zero-sized — it exists purely to be a distinct type the
/// compiler can check, not to hold data.
pub trait EntityMarker: 'static {
    /// The owning module's name, exactly as declared in the manifest.
    const MODULE: &'static str;
    /// The entity's own name, exactly as declared in the manifest.
    const NAME: &'static str;

    fn qualified_name() -> String {
        format!("{}.{}", Self::MODULE, Self::NAME)
    }
}

/// Implemented by a generated marker type for one attribute of one entity
/// (e.g. `markers::Sales::Order_Number`, generated from a manifest entity's
/// `attributes` list).
pub trait AttributeMarker: 'static {
    type Entity: EntityMarker;
    const NAME: &'static str;
}

/// A typed reference to an entity, carrying no runtime state beyond what's
/// needed to resolve back to the qualified name `mxrs-writer` already
/// expects (`"Module.Entity"`) — so once `mxrs-dsl` is wired to accept
/// `Ref<M>` in place of a raw target string, existing target-resolution
/// code in `mxrs-writer::domain` (which already branches on a `.` in the
/// string) needs no changes at all.
#[derive(Debug)]
pub struct Ref<M: EntityMarker>(PhantomData<M>);

impl<M: EntityMarker> Ref<M> {
    pub fn new() -> Self {
        Ref(PhantomData)
    }

    pub fn qualified_name(&self) -> String {
        M::qualified_name()
    }
}

impl<M: EntityMarker> Default for Ref<M> {
    fn default() -> Self {
        Self::new()
    }
}

impl<M: EntityMarker> Clone for Ref<M> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: EntityMarker> Copy for Ref<M> {}

#[cfg(test)]
mod tests {
    use super::*;

    struct Order;
    impl EntityMarker for Order {
        const MODULE: &'static str = "Sales";
        const NAME: &'static str = "Order";
    }

    struct OrderNumber;
    impl AttributeMarker for OrderNumber {
        type Entity = Order;
        const NAME: &'static str = "Number";
    }

    #[test]
    fn qualified_name_joins_module_and_entity() {
        assert_eq!(Order::qualified_name(), "Sales.Order");
    }

    #[test]
    fn ref_resolves_to_its_marker_qualified_name() {
        let r: Ref<Order> = Ref::new();
        assert_eq!(r.qualified_name(), "Sales.Order");
    }

    #[test]
    fn attribute_marker_names_its_owning_entity() {
        assert_eq!(OrderNumber::NAME, "Number");
        assert_eq!(
            <OrderNumber as AttributeMarker>::Entity::qualified_name(),
            "Sales.Order"
        );
    }
}
