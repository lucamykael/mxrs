//! Phase 4 marker-type contract: the trait surface `mxrs-typegen`-generated
//! code implements, and the newtype `mxrs-dsl` (eventually) uses in place of
//! raw string references. Kept here rather than in `mxrs-typegen` itself so
//! a user's generated-markers module only needs to depend on `mxrs-ir`
//! (already a transitive dependency via `mxrs-dsl`), not on the codegen
//! tool — the same "no capability exists only through generated code" split
//! `mxrs-macros` (Phase 8) is required to follow for the same reason.
//!
//! **Scope of this pass**: the trait contract, the generated code that
//! implements it (`mxrs-typegen`), and a `Ref<M>` newtype exist and are
//! tested (including a `trybuild` compile-fail proving an unknown reference
//! doesn't compile — the plan's literal Phase 4 done-definition). `mxrs-dsl`
//! itself is **not yet wired** to require `Ref<M>` instead of a plain
//! `&str` in `EntityBuilder::association`/`association_target` — that's a
//! larger, separate change to the builder's public API (touches every
//! existing association-target call site and `mxrs-writer`'s
//! `AssociationDecl::target` resolution), left for a follow-up pass so this
//! one doesn't destabilize Phase 3's already-shipped, oracle-verified
//! surface.

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
        assert_eq!(<OrderNumber as AttributeMarker>::Entity::qualified_name(), "Sales.Order");
    }
}
