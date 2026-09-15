use mxrs_ir::declaration::{AccessMemberKind, AccessRuleDecl, MemberAccessDecl, MemberRights};
use mxrs_ir::{AssociationMarker, AttributeMarker, EntityMarker};

/// Builder for one entity access rule.
///
/// Members are named by marker type rather than by string, so a rule that
/// grants rights on a renamed or deleted attribute fails `cargo build` instead
/// of silently granting nothing — the same reason `EntityBuilder::association`
/// takes an `AssociationMarker`. The rule's roles stay plain strings: module
/// roles have no marker surface (they are declared by
/// `ModuleBuilder::role`, and the writer already rejects an unknown one).
pub struct AccessRuleBuilder {
    decl: AccessRuleDecl,
}

impl AccessRuleBuilder {
    pub(crate) fn new(roles: Vec<String>) -> Self {
        Self {
            decl: AccessRuleDecl::new(roles),
        }
    }

    pub(crate) fn into_decl(self) -> AccessRuleDecl {
        self.decl
    }

    pub fn documentation(&mut self, text: impl Into<String>) -> &mut Self {
        self.decl.documentation = text.into();
        self
    }

    pub fn allow_create(&mut self, allow: bool) -> &mut Self {
        self.decl.allow_create = allow;
        self
    }

    pub fn allow_delete(&mut self, allow: bool) -> &mut Self {
        self.decl.allow_delete = allow;
        self
    }

    /// Rights for every member this rule does not name explicitly. Defaults
    /// to [`MemberRights::None`]: a rule that says nothing about a member
    /// grants nothing on it.
    pub fn default_rights(&mut self, rights: MemberRights) -> &mut Self {
        self.decl.default_rights = rights;
        self
    }

    /// Restricts the rule to objects matching an XPath constraint. Stored
    /// verbatim; mxrs has no XPath parser, so an invalid constraint surfaces
    /// in Studio Pro rather than here.
    pub fn xpath(&mut self, constraint: impl Into<String>) -> &mut Self {
        self.decl.xpath_constraint = constraint.into();
        self
    }

    pub fn xpath_caption(&mut self, caption: impl Into<String>) -> &mut Self {
        self.decl.xpath_caption = Some(caption.into());
        self
    }

    /// Grants `rights` on one attribute of the entity this rule belongs to.
    pub fn attribute<A: AttributeMarker>(&mut self, rights: MemberRights) -> &mut Self {
        let reference = format!("{}.{}", A::Entity::qualified_name(), A::NAME);
        self.push(reference, AccessMemberKind::Attribute, rights)
    }

    /// Grants `rights` on one association. The native document stores the
    /// association's own qualified name (`"Module.Association"`), not a path
    /// through the entity.
    pub fn association<A: AssociationMarker>(&mut self, rights: MemberRights) -> &mut Self {
        let reference = format!("{}.{}", A::From::MODULE, A::NAME);
        self.push(reference, AccessMemberKind::Association, rights)
    }

    /// Last declaration wins for a member named twice: the native document
    /// has one `MemberAccess` per member, and keeping both would leave which
    /// one applies up to array order.
    fn push(
        &mut self,
        reference: String,
        kind: AccessMemberKind,
        rights: MemberRights,
    ) -> &mut Self {
        self.decl
            .members
            .retain(|member| member.reference != reference);
        self.decl.members.push(MemberAccessDecl {
            reference,
            kind,
            rights,
        });
        self
    }
}
