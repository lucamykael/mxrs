use mxrs_ir::declaration::{
    AssociationDecl, AssociationOwner, AssociationStorage, AttributeDecl, AttributeType,
    EntityDecl, EntityIndexDecl, EntityInheritanceDecl, IndexMemberDecl, LifecycleDecl,
    LifecycleEvent, SystemMember, SystemMembersDecl,
};
use mxrs_ir::{AssociationMarker, AttributeMarker, EntityMarker, MicroflowMarker};

use crate::access::AccessRuleBuilder;

pub struct EntityBuilder {
    decl: EntityDecl,
}

/// Configures the system-owned members of a root entity. All flags start
/// disabled, so the closure names only the members the application wants.
pub struct SystemMembersBuilder {
    members: SystemMembersDecl,
}

impl SystemMembersBuilder {
    pub fn owner(&mut self, enabled: bool) -> &mut Self {
        self.members.owner = enabled;
        self
    }

    pub fn created_date(&mut self, enabled: bool) -> &mut Self {
        self.members.created_date = enabled;
        self
    }

    pub fn changed_date(&mut self, enabled: bool) -> &mut Self {
        self.members.changed_date = enabled;
        self
    }

    pub fn changed_by(&mut self, enabled: bool) -> &mut Self {
        self.members.changed_by = enabled;
        self
    }
}

/// Builds one ordered Mendix entity index without exposing attribute names as
/// strings. Attribute members are checked by Rust's marker traits; system
/// members are a closed enum.
pub struct EntityIndexBuilder {
    index: EntityIndexDecl,
}

impl EntityIndexBuilder {
    pub fn attribute<A: AttributeMarker>(&mut self) -> &mut Self {
        self.index.members.push(IndexMemberDecl::Attribute {
            name: A::NAME.to_string(),
            ascending: true,
        });
        self
    }

    pub fn attribute_descending<A: AttributeMarker>(&mut self) -> &mut Self {
        self.index.members.push(IndexMemberDecl::Attribute {
            name: A::NAME.to_string(),
            ascending: false,
        });
        self
    }

    pub fn system(&mut self, member: SystemMember) -> &mut Self {
        self.index.members.push(IndexMemberDecl::System {
            member,
            ascending: true,
        });
        self
    }

    pub fn system_descending(&mut self, member: SystemMember) -> &mut Self {
        self.index.members.push(IndexMemberDecl::System {
            member,
            ascending: false,
        });
        self
    }

    pub fn include_offline(&mut self, enabled: bool) -> &mut Self {
        self.index.include_offline = enabled;
        self
    }
}

/// Options shared by every lifecycle callback. Before callbacks default to
/// raising when the microflow returns false, matching Studio Pro; after
/// callbacks default to not raising.
pub struct LifecycleBuilder {
    callback: LifecycleDecl,
}

impl LifecycleBuilder {
    pub fn pass_event_object(&mut self, enabled: bool) -> &mut Self {
        self.callback.pass_event_object = enabled;
        self
    }

    pub fn raise_error_on_false(&mut self, enabled: bool) -> &mut Self {
        self.callback.raise_error_on_false = enabled;
        self
    }
}

impl EntityBuilder {
    pub(crate) fn new(name: impl Into<String>) -> Self {
        EntityBuilder {
            decl: EntityDecl::new(name),
        }
    }

    pub(crate) fn into_decl(self) -> EntityDecl {
        self.decl
    }

    pub fn documentation(&mut self, text: impl Into<String>) -> &mut Self {
        self.decl.documentation = text.into();
        self
    }

    pub fn persistable(&mut self, value: bool) -> &mut Self {
        self.decl.persistable = value;
        self
    }

    /// Makes inheritance authoritative and declares a root entity with no
    /// system-owned members.
    pub fn root(&mut self) -> &mut Self {
        self.decl.inheritance = Some(EntityInheritanceDecl::Root(SystemMembersDecl::default()));
        self
    }

    /// Makes inheritance authoritative and declares a root entity's system
    /// members. Calling this after [`EntityBuilder::generalizes`] replaces
    /// that parent declaration, and vice versa.
    pub fn system_members(
        &mut self,
        configure: impl FnOnce(&mut SystemMembersBuilder),
    ) -> &mut Self {
        let mut builder = SystemMembersBuilder {
            members: SystemMembersDecl::default(),
        };
        configure(&mut builder);
        self.decl.inheritance = Some(EntityInheritanceDecl::Root(builder.members));
        self
    }

    /// Declares an entity parent through an [`EntityMarker`], so a renamed or
    /// missing parent fails at `cargo build` rather than during `.mpr` write.
    pub fn generalizes<E: EntityMarker>(&mut self) -> &mut Self {
        self.decl.inheritance = Some(EntityInheritanceDecl::Generalizes(E::qualified_name()));
        self
    }

    /// Adds one authoritative index. The first call takes ownership of the
    /// imported index list; use [`EntityBuilder::clear_indexes`] to express an
    /// authoritative empty list.
    pub fn index(&mut self, configure: impl FnOnce(&mut EntityIndexBuilder)) -> &mut Self {
        let mut builder = EntityIndexBuilder {
            index: EntityIndexDecl::new(),
        };
        configure(&mut builder);
        self.decl
            .indexes
            .get_or_insert_with(Vec::new)
            .push(builder.index);
        self
    }

    pub fn clear_indexes(&mut self) -> &mut Self {
        self.decl.indexes = Some(vec![]);
        self
    }

    pub fn before_commit<M: MicroflowMarker>(
        &mut self,
        configure: impl FnOnce(&mut LifecycleBuilder),
    ) -> &mut Self {
        self.lifecycle::<M>(LifecycleEvent::BeforeCommit, configure)
    }

    pub fn after_commit<M: MicroflowMarker>(
        &mut self,
        configure: impl FnOnce(&mut LifecycleBuilder),
    ) -> &mut Self {
        self.lifecycle::<M>(LifecycleEvent::AfterCommit, configure)
    }

    pub fn before_delete<M: MicroflowMarker>(
        &mut self,
        configure: impl FnOnce(&mut LifecycleBuilder),
    ) -> &mut Self {
        self.lifecycle::<M>(LifecycleEvent::BeforeDelete, configure)
    }

    pub fn after_delete<M: MicroflowMarker>(
        &mut self,
        configure: impl FnOnce(&mut LifecycleBuilder),
    ) -> &mut Self {
        self.lifecycle::<M>(LifecycleEvent::AfterDelete, configure)
    }

    pub fn clear_lifecycle(&mut self) -> &mut Self {
        self.decl.lifecycle = Some(vec![]);
        self
    }

    fn lifecycle<M: MicroflowMarker>(
        &mut self,
        event: LifecycleEvent,
        configure: impl FnOnce(&mut LifecycleBuilder),
    ) -> &mut Self {
        let mut builder = LifecycleBuilder {
            callback: LifecycleDecl {
                event,
                handler: M::qualified_name(),
                pass_event_object: true,
                raise_error_on_false: matches!(
                    event,
                    LifecycleEvent::BeforeCommit | LifecycleEvent::BeforeDelete
                ),
            },
        };
        configure(&mut builder);
        self.decl
            .lifecycle
            .get_or_insert_with(Vec::new)
            .push(builder.callback);
        self
    }

    /// Declares one entity access rule for `roles`, making this entity's
    /// access rules authoritative.
    ///
    /// Calling it at all switches the entity from "preserve whatever the
    /// imported model had" to "these rules and no others", so an imported
    /// entity keeps its rules until the declaration takes them over — the
    /// same contract `ModuleBuilder::role` has for module security. Use
    /// [`EntityBuilder::clear_access_rules`] to declare that an entity has
    /// none.
    pub fn access_rule(
        &mut self,
        roles: impl IntoIterator<Item = impl Into<String>>,
        configure: impl FnOnce(&mut AccessRuleBuilder),
    ) -> &mut Self {
        let mut builder =
            AccessRuleBuilder::new(roles.into_iter().map(Into::into).collect::<Vec<_>>());
        configure(&mut builder);
        self.decl
            .access_rules
            .get_or_insert_with(Vec::new)
            .push(builder.into_decl());
        self
    }

    /// Declares that this entity has no access rules, dropping any the
    /// imported model carried.
    pub fn clear_access_rules(&mut self) -> &mut Self {
        self.decl.access_rules = Some(vec![]);
        self
    }

    pub fn string(&mut self, name: impl Into<String>) -> &mut AttributeDecl {
        self.push_attribute(name, AttributeType::String)
    }

    pub fn integer(&mut self, name: impl Into<String>) -> &mut AttributeDecl {
        self.push_attribute(name, AttributeType::Integer)
    }

    pub fn long(&mut self, name: impl Into<String>) -> &mut AttributeDecl {
        self.push_attribute(name, AttributeType::Long)
    }

    pub fn float(&mut self, name: impl Into<String>) -> &mut AttributeDecl {
        self.push_attribute(name, AttributeType::Float)
    }

    pub fn decimal(&mut self, name: impl Into<String>) -> &mut AttributeDecl {
        self.push_attribute(name, AttributeType::Decimal)
    }

    pub fn boolean(&mut self, name: impl Into<String>) -> &mut AttributeDecl {
        self.push_attribute(name, AttributeType::Boolean)
    }

    pub fn datetime(&mut self, name: impl Into<String>) -> &mut AttributeDecl {
        self.push_attribute(name, AttributeType::DateTime)
    }

    pub fn autonumber(&mut self, name: impl Into<String>) -> &mut AttributeDecl {
        self.push_attribute(name, AttributeType::AutoNumber)
    }

    pub fn hash_string(&mut self, name: impl Into<String>) -> &mut AttributeDecl {
        self.push_attribute(name, AttributeType::HashString)
    }

    pub fn binary(&mut self, name: impl Into<String>) -> &mut AttributeDecl {
        self.push_attribute(name, AttributeType::Binary)
    }

    pub fn enumeration(
        &mut self,
        name: impl Into<String>,
        enumeration: impl Into<String>,
    ) -> &mut AttributeDecl {
        let attribute = self.push_attribute(name, AttributeType::Enumeration);
        attribute.enumeration = Some(enumeration.into());
        attribute
    }

    /// Declares an association from this entity, identified entirely by
    /// `A` — a compile-time marker type implementing
    /// `mxrs_ir::AssociationMarker` (generated by `mxrs-typegen` from a
    /// schema manifest, or auto-generated by `project! {}` from an inline
    /// `association` statement; see `mxrs_ir::markers`). Name, target, and
    /// association type are all read off `A`'s associated consts/types —
    /// there is no raw-string or raw-`AssociationType` parameter to get out
    /// of sync with the manifest, closing the gap `AssociationMarker`'s own
    /// doc comment used to describe as deliberately left open. A typo'd or
    /// renamed target still fails `cargo build` (via `A::To: EntityMarker`)
    /// instead of `mxrs-writer`'s runtime
    /// `UnknownAssociationTarget`/`UnknownCrossModuleAssociationTarget`
    /// (which still runs too — see `mxrs_ir::markers`' doc comment for why
    /// both checks matter). Works identically for a same-module or
    /// cross-module target: `A::To::qualified_name()` always resolves to
    /// `"Module.Entity"`, and `mxrs-writer` routes on the resolved module,
    /// not on whether the string happens to contain a dot.
    ///
    /// Deliberately not checked here (same reasoning `Ref<M>` already
    /// documents for the target-only case): that `A::From` actually names
    /// *this* entity. `EntityBuilder` has no compile-time or runtime handle
    /// on its own qualified name at this point in construction — enforcing
    /// that would mean making the whole `ProjectBuilder` invocation
    /// type-level, out of scope for this pass, same boundary `Ref<M>`
    /// already draws.
    ///
    /// Defaults to `AssociationOwner::Default`/`AssociationStorage::Column`; set the
    /// returned `AssociationDecl`'s public fields directly to override.
    pub fn association<A: AssociationMarker>(&mut self) -> &mut AssociationDecl {
        self.decl.associations.push(AssociationDecl {
            name: A::NAME.to_string(),
            target: A::To::qualified_name(),
            association_type: A::ASSOCIATION_TYPE,
            owner: AssociationOwner::Default,
            storage: AssociationStorage::Column,
            documentation: String::new(),
        });
        self.decl.associations.last_mut().expect("just pushed")
    }

    fn push_attribute(
        &mut self,
        name: impl Into<String>,
        attribute_type: AttributeType,
    ) -> &mut AttributeDecl {
        self.decl
            .attributes
            .push(AttributeDecl::new(name, attribute_type));
        self.decl.attributes.last_mut().expect("just pushed")
    }
}
