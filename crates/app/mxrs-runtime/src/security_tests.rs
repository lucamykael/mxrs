use super::*;
use serde_json::json;

fn user() -> SecurityContext {
    SecurityContext {
        user: Some("user".into()),
        user_roles: BTreeSet::from(["User".into(), "Unmapped".into()]),
        ..Default::default()
    }
}

fn policy() -> SecurityPolicy {
    SecurityPolicy {
        enabled: true,
        administrator_roles: BTreeSet::from(["Administrator".into()]),
        user_role_modules: BTreeMap::from([("User".into(), BTreeSet::from(["Sales.User".into()]))]),
        documents: BTreeMap::from([
            (
                "Sales.Allowed".into(),
                BTreeSet::from(["Sales.User".into()]),
            ),
            ("Sales.Empty".into(), BTreeSet::new()),
        ]),
        ..Default::default()
    }
}

#[test]
fn disabled_security_and_explicit_administrators_bypass_rules_but_anonymous_users_do_not() {
    let mut policy = policy();
    let admin = SecurityContext {
        user_roles: BTreeSet::from(["Administrator".into()]),
        ..Default::default()
    };
    for action in [
        EntityAction::Create,
        EntityAction::Read,
        EntityAction::Write,
        EntityAction::Delete,
    ] {
        assert!(policy.entity_allowed("Missing", action, Some("Missing"), None, &admin));
        assert!(!policy.entity_allowed("Missing", action, None, None, &SecurityContext::default()));
    }
    assert!(policy.document_allowed("Missing", &admin));
    assert!(!policy.document_allowed("Sales.Allowed", &SecurityContext::default()));
    assert!(!policy.document_allowed("Sales.Empty", &user()));
    policy.enabled = false;
    assert!(policy.document_allowed("Missing", &SecurityContext::default()));
    assert!(policy.entity_allowed(
        "Missing",
        EntityAction::Write,
        None,
        None,
        &SecurityContext::default()
    ));
}

#[test]
fn role_expansion_preserves_trusted_module_roles_and_does_not_invent_unmapped_roles() {
    let policy = policy();
    let mut context = user();
    context.module_roles.insert("Other.Trusted".into());
    let expanded = policy.expand_context(context.clone());
    assert_eq!(
        expanded.module_roles,
        BTreeSet::from(["Other.Trusted".into(), "Sales.User".into()])
    );
    assert_eq!(expanded.user, context.user);
    assert!(policy.authorize_document("Sales.Allowed", &context).is_ok());
    assert_eq!(
        policy.authorize_document("Sales.Denied", &context),
        Err(RuntimeError::NotAuthorized {
            action: "execute".into(),
            resource: "Sales.Denied".into()
        })
    );
}

#[test]
fn entity_access_is_the_union_of_matching_rules_with_explicit_member_overrides() {
    let mut policy = policy();
    policy.entities.insert(
        "Sales.Order".into(),
        vec![
            EntityRule {
                module_roles: BTreeSet::from(["Wrong.Role".into()]),
                create: true,
                delete: true,
                default_member_right: Some(MemberRight::Write),
                ..Default::default()
            },
            EntityRule {
                module_roles: BTreeSet::from(["Sales.User".into()]),
                create: true,
                default_member_right: Some(MemberRight::Read),
                member_rights: BTreeMap::from([
                    ("Hidden".into(), MemberRight::None),
                    ("Editable".into(), MemberRight::Write),
                ]),
                ..Default::default()
            },
        ],
    );
    let context = user();
    for (action, member, allowed) in [
        (EntityAction::Create, None, true),
        (EntityAction::Delete, None, false),
        (EntityAction::Read, None, true),
        (EntityAction::Read, Some("Default"), true),
        (EntityAction::Read, Some("Hidden"), false),
        (EntityAction::Read, Some("Editable"), true),
        (EntityAction::Write, None, true),
        (EntityAction::Write, Some("Default"), false),
        (EntityAction::Write, Some("Hidden"), false),
        (EntityAction::Write, Some("Editable"), true),
    ] {
        assert_eq!(
            policy.entity_allowed("Sales.Order", action, member, None, &context),
            allowed,
            "{action:?} {member:?}"
        );
    }
    policy
        .entities
        .get_mut("Sales.Order")
        .unwrap()
        .push(EntityRule {
            module_roles: BTreeSet::from(["Sales.User".into()]),
            delete: true,
            member_rights: BTreeMap::from([("Hidden".into(), MemberRight::Read)]),
            ..Default::default()
        });
    assert!(policy.entity_allowed("Sales.Order", EntityAction::Delete, None, None, &context));
    assert!(policy.entity_allowed(
        "Sales.Order",
        EntityAction::Read,
        Some("Hidden"),
        None,
        &context
    ));
    assert!(!policy.entity_allowed(
        "Sales.Order",
        EntityAction::Read,
        None,
        None,
        &SecurityContext::default()
    ));
}

#[test]
fn absent_member_rights_never_grant_reads_or_writes_and_default_write_can_be_narrowed() {
    let mut policy = policy();
    let context = user();
    policy.entities.insert(
        "Sales.Order".into(),
        vec![EntityRule {
            module_roles: BTreeSet::from(["Sales.User".into()]),
            member_rights: BTreeMap::from([("ReadOnly".into(), MemberRight::Read)]),
            ..Default::default()
        }],
    );
    for (action, member) in [
        (EntityAction::Create, None),
        (EntityAction::Read, Some("Unspecified")),
        (EntityAction::Write, Some("Unspecified")),
        (EntityAction::Write, None),
    ] {
        assert!(!policy.entity_allowed("Sales.Order", action, member, None, &context));
    }
    policy.entities.get_mut("Sales.Order").unwrap()[0].default_member_right =
        Some(MemberRight::Write);
    assert!(policy.entity_allowed("Sales.Order", EntityAction::Write, None, None, &context));
    assert!(policy.entity_allowed(
        "Sales.Order",
        EntityAction::Write,
        Some("Default"),
        None,
        &context
    ));
    assert!(!policy.entity_allowed(
        "Sales.Order",
        EntityAction::Write,
        Some("ReadOnly"),
        None,
        &context
    ));
}

#[test]
fn runtime_authorization_precedes_lookup_and_action_failures_restore_committed_state() {
    let schema = StoreSchema::default().entity("Sales.Order", BTreeMap::new(), false);
    let mut runtime = Runtime::new(Store::new(schema), policy());
    assert!(matches!(
        runtime.invoke("Missing", &Value::Null, &SecurityContext::default()),
        Err(RuntimeError::NotAuthorized { .. })
    ));
    assert_eq!(
        runtime.invoke("Sales.Allowed", &Value::Null, &user()),
        Err(RuntimeError::UnknownAction("Sales.Allowed".into()))
    );
    runtime.register_action("Sales.Allowed", |store: &mut Store, arguments: &Value| {
        let order = store.create("Sales.Order")?;
        store.commit("Sales.Order", &order.id)?;
        if arguments["fail"] == true {
            Err(RuntimeError::Transaction("action failed".into()))
        } else {
            Ok(json!(order.id))
        }
    });
    assert!(
        runtime
            .invoke("Sales.Allowed", &json!({"fail":true}), &user())
            .is_err()
    );
    assert!(runtime.store().persistent_objects().is_empty());
    let id = runtime
        .invoke("Sales.Allowed", &json!({"fail":false}), &user())
        .unwrap();
    assert_eq!(runtime.store().persistent_objects().len(), 1);
    runtime
        .store_mut()
        .delete("Sales.Order", id.as_str().unwrap())
        .unwrap();
    assert!(runtime.store().persistent_objects().is_empty());
}

#[test]
fn runtime_panics_restore_state_before_propagating_to_the_http_boundary() {
    let schema = StoreSchema::default().entity("Sales.Order", BTreeMap::new(), false);
    let mut runtime = Runtime::new(Store::new(schema), policy());
    runtime.register_action(
        "Sales.Allowed",
        |store: &mut Store, _: &Value| -> Result<Value> {
            let order = store.create("Sales.Order")?;
            store.commit("Sales.Order", &order.id)?;
            panic!("native action panic")
        },
    );
    assert!(
        catch_unwind(AssertUnwindSafe(|| runtime.invoke(
            "Sales.Allowed",
            &Value::Null,
            &user()
        )))
        .is_err()
    );
    assert!(runtime.store().persistent_objects().is_empty());
}
