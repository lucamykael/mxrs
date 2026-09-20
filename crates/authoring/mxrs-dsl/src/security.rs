use mxrs_ir::{DemoUserDecl, PasswordPolicyDecl, ProjectSecurityDecl, SecurityLevel, UserRoleDecl};

pub struct SecurityBuilder {
    declaration: ProjectSecurityDecl,
}

impl SecurityBuilder {
    pub(crate) fn new() -> Self {
        Self {
            declaration: ProjectSecurityDecl::default(),
        }
    }

    pub(crate) fn into_decl(self) -> ProjectSecurityDecl {
        self.declaration
    }

    pub fn level(&mut self, level: SecurityLevel) -> &mut Self {
        self.declaration.level = level;
        self
    }

    pub fn check_security(&mut self, value: bool) -> &mut Self {
        self.declaration.check_security = value;
        self
    }

    pub fn admin_role(&mut self, name: impl Into<String>) -> &mut Self {
        self.declaration.admin_user_role = name.into();
        self
    }

    pub fn guest_access(&mut self, role: impl Into<String>) -> &mut Self {
        self.declaration.guest_user_role = Some(role.into());
        self
    }

    pub fn disable_guest_access(&mut self) -> &mut Self {
        self.declaration.guest_user_role = None;
        self
    }

    pub fn sign_in_microflow(&mut self, qualified_name: impl Into<String>) -> &mut Self {
        self.declaration.sign_in_microflow = Some(qualified_name.into());
        self
    }

    pub fn clear_roles(&mut self) -> &mut Self {
        self.declaration.user_roles.clear();
        self
    }

    pub fn role(
        &mut self,
        name: impl Into<String>,
        configure: impl FnOnce(&mut UserRoleBuilder),
    ) -> &mut Self {
        let mut builder = UserRoleBuilder::new(name);
        configure(&mut builder);
        self.declaration.user_roles.push(builder.into_decl());
        self
    }

    pub fn password_policy(
        &mut self,
        configure: impl FnOnce(&mut PasswordPolicyDecl),
    ) -> &mut Self {
        configure(&mut self.declaration.password_policy);
        self
    }

    pub fn demo_user(
        &mut self,
        name: impl Into<String>,
        configure: impl FnOnce(&mut DemoUserBuilder),
    ) -> &mut Self {
        let mut builder = DemoUserBuilder::new(name);
        configure(&mut builder);
        self.declaration.demo_users.push(builder.into_decl());
        self
    }
}

pub struct DemoUserBuilder {
    declaration: DemoUserDecl,
}

impl DemoUserBuilder {
    fn new(name: impl Into<String>) -> Self {
        Self {
            declaration: DemoUserDecl::new(name),
        }
    }

    fn into_decl(self) -> DemoUserDecl {
        self.declaration
    }

    pub fn entity(&mut self, qualified_name: impl Into<String>) -> &mut Self {
        self.declaration.entity = qualified_name.into();
        self
    }

    pub fn role(&mut self, name: impl Into<String>) -> &mut Self {
        self.declaration.roles.push(name.into());
        self
    }

    /// Names the environment variable the writer reads the password from.
    /// The value itself never enters the declaration or generated source.
    pub fn password_from_env(&mut self, variable: impl Into<String>) -> &mut Self {
        self.declaration.password_env = Some(variable.into());
        self
    }
}

pub struct UserRoleBuilder {
    declaration: UserRoleDecl,
}

impl UserRoleBuilder {
    fn new(name: impl Into<String>) -> Self {
        Self {
            declaration: UserRoleDecl::new(name),
        }
    }

    fn into_decl(self) -> UserRoleDecl {
        self.declaration
    }

    pub fn description(&mut self, value: impl Into<String>) -> &mut Self {
        self.declaration.description = value.into();
        self
    }

    pub fn administrator(&mut self, value: bool) -> &mut Self {
        self.declaration.administrator = value;
        self
    }

    pub fn check_security(&mut self, value: bool) -> &mut Self {
        self.declaration.check_security = value;
        self
    }

    pub fn manage_users_without_roles(&mut self, value: bool) -> &mut Self {
        self.declaration.manage_users_without_roles = value;
        self
    }

    pub fn manageable_role(&mut self, name: impl Into<String>) -> &mut Self {
        self.declaration.manageable_roles.push(name.into());
        self
    }

    pub fn module_role(&mut self, qualified_name: impl Into<String>) -> &mut Self {
        self.declaration.module_roles.push(qualified_name.into());
        self
    }
}
