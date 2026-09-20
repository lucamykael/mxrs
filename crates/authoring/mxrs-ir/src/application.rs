//! Project-wide security/navigation declarations. These are semantic Rust
//! values; native BSON identity and compatibility fields belong to the writer.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SecurityLevel {
    #[default]
    CheckNothing,
    CheckFormsAndMicroflows,
    CheckEverything,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordPolicyDecl {
    pub minimum_length: i32,
    pub require_digit: bool,
    pub require_mixed_case: bool,
    pub require_symbol: bool,
}

impl Default for PasswordPolicyDecl {
    fn default() -> Self {
        Self {
            minimum_length: 6,
            require_digit: true,
            require_mixed_case: true,
            require_symbol: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRoleDecl {
    pub name: String,
    pub description: String,
    pub administrator: bool,
    pub check_security: bool,
    pub manage_users_without_roles: bool,
    pub manageable_roles: Vec<String>,
    pub module_roles: Vec<String>,
}

impl UserRoleDecl {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            administrator: false,
            check_security: true,
            manage_users_without_roles: false,
            manageable_roles: vec![],
            module_roles: vec![],
        }
    }
}

/// A local demo account stored in the project's `Security$ProjectSecurity`
/// unit. The password is never part of the declaration: `password_env` names
/// an environment variable the writer resolves at write time, following the
/// sensitive-constant rule that credential values never appear in source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DemoUserDecl {
    pub name: String,
    /// Qualified user entity, `System.User` by default.
    pub entity: String,
    /// Project user roles granted to the demo account.
    pub roles: Vec<String>,
    /// Environment variable holding the password. When the variable is
    /// absent, an already-stored password is preserved; a brand-new demo
    /// user without a resolvable password fails closed.
    pub password_env: Option<String>,
}

impl DemoUserDecl {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            entity: "System.User".to_string(),
            roles: vec![],
            password_env: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSecurityDecl {
    pub level: SecurityLevel,
    pub check_security: bool,
    pub admin_user_role: String,
    pub guest_user_role: Option<String>,
    pub sign_in_microflow: Option<String>,
    pub user_roles: Vec<UserRoleDecl>,
    pub demo_users: Vec<DemoUserDecl>,
    pub password_policy: PasswordPolicyDecl,
}

impl Default for ProjectSecurityDecl {
    fn default() -> Self {
        let mut administrator = UserRoleDecl::new("Administrator");
        administrator.administrator = true;
        administrator
            .module_roles
            .push("System.Administrator".to_string());
        Self {
            level: SecurityLevel::CheckNothing,
            check_security: true,
            admin_user_role: "Administrator".to_string(),
            guest_user_role: None,
            sign_in_microflow: None,
            user_roles: vec![administrator],
            demo_users: vec![],
            password_policy: PasswordPolicyDecl::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavigationIconDecl {
    Glyph(String),
    Code(i64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleRoleDecl {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NavigationItemDecl {
    pub caption: std::collections::BTreeMap<String, String>,
    pub page: Option<String>,
    pub microflow: Option<String>,
    pub icon: Option<NavigationIconDecl>,
    pub items: Vec<NavigationItemDecl>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoleHomeDecl {
    pub user_role: String,
    pub page: Option<String>,
    pub microflow: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationProfileDecl {
    pub name: String,
    pub kind: String,
    pub app_title: std::collections::BTreeMap<String, String>,
    pub home_page: Option<String>,
    pub home_microflow: Option<String>,
    pub sign_in_page: Option<String>,
    pub role_homes: Vec<RoleHomeDecl>,
    pub items: Vec<NavigationItemDecl>,
}

impl NavigationProfileDecl {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: "Responsive".to_string(),
            app_title: Default::default(),
            home_page: None,
            home_microflow: None,
            sign_in_page: None,
            role_homes: vec![],
            items: vec![],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NavigationDecl {
    pub profiles: Vec<NavigationProfileDecl>,
}

impl ModuleRoleDecl {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
        }
    }
}
