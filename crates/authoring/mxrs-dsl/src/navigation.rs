use mxrs_ir::{NavigationDecl, NavigationItemDecl, NavigationProfileDecl, RoleHomeDecl};

pub struct NavigationBuilder {
    declaration: NavigationDecl,
}

impl NavigationBuilder {
    pub(crate) fn new() -> Self {
        Self {
            declaration: NavigationDecl::default(),
        }
    }

    pub(crate) fn into_decl(self) -> NavigationDecl {
        self.declaration
    }

    pub fn profile(
        &mut self,
        name: impl Into<String>,
        configure: impl FnOnce(&mut NavigationProfileBuilder),
    ) -> &mut Self {
        let mut builder = NavigationProfileBuilder::new(name);
        configure(&mut builder);
        self.declaration.profiles.push(builder.into_decl());
        self
    }
}

pub struct NavigationProfileBuilder {
    declaration: NavigationProfileDecl,
}

impl NavigationProfileBuilder {
    fn new(name: impl Into<String>) -> Self {
        Self {
            declaration: NavigationProfileDecl::new(name),
        }
    }
    fn into_decl(self) -> NavigationProfileDecl {
        self.declaration
    }

    pub fn kind(&mut self, value: impl Into<String>) -> &mut Self {
        self.declaration.kind = value.into();
        self
    }
    pub fn offline(&mut self) -> &mut Self {
        self.declaration.kind = "Offline".to_string();
        self
    }
    pub fn title(&mut self, locale: impl Into<String>, text: impl Into<String>) -> &mut Self {
        self.declaration
            .app_title
            .insert(locale.into(), text.into());
        self
    }
    pub fn home_page(&mut self, reference: impl Into<String>) -> &mut Self {
        self.declaration.home_page = Some(reference.into());
        self.declaration.home_microflow = None;
        self
    }
    pub fn home_microflow(&mut self, reference: impl Into<String>) -> &mut Self {
        self.declaration.home_microflow = Some(reference.into());
        self.declaration.home_page = None;
        self
    }
    pub fn sign_in_page(&mut self, reference: impl Into<String>) -> &mut Self {
        self.declaration.sign_in_page = Some(reference.into());
        self
    }

    pub fn home_for_page(
        &mut self,
        user_role: impl Into<String>,
        page: impl Into<String>,
    ) -> &mut Self {
        self.declaration.role_homes.push(RoleHomeDecl {
            user_role: user_role.into(),
            page: Some(page.into()),
            microflow: None,
        });
        self
    }

    pub fn home_for_microflow(
        &mut self,
        user_role: impl Into<String>,
        microflow: impl Into<String>,
    ) -> &mut Self {
        self.declaration.role_homes.push(RoleHomeDecl {
            user_role: user_role.into(),
            page: None,
            microflow: Some(microflow.into()),
        });
        self
    }

    pub fn item(
        &mut self,
        caption: impl Into<String>,
        configure: impl FnOnce(&mut NavigationItemBuilder),
    ) -> &mut Self {
        let mut builder = NavigationItemBuilder::new(caption);
        configure(&mut builder);
        self.declaration.items.push(builder.into_decl());
        self
    }
}

pub struct NavigationItemBuilder {
    declaration: NavigationItemDecl,
}

impl NavigationItemBuilder {
    fn new(caption: impl Into<String>) -> Self {
        let mut declaration = NavigationItemDecl::default();
        declaration
            .caption
            .insert("en_US".to_string(), caption.into());
        Self { declaration }
    }

    fn into_decl(self) -> NavigationItemDecl {
        self.declaration
    }
    pub fn caption(&mut self, locale: impl Into<String>, text: impl Into<String>) -> &mut Self {
        self.declaration.caption.insert(locale.into(), text.into());
        self
    }
    pub fn page(&mut self, reference: impl Into<String>) -> &mut Self {
        self.declaration.page = Some(reference.into());
        self.declaration.microflow = None;
        self
    }
    pub fn microflow(&mut self, reference: impl Into<String>) -> &mut Self {
        self.declaration.microflow = Some(reference.into());
        self.declaration.page = None;
        self
    }
    pub fn icon(&mut self, code: impl Into<String>) -> &mut Self {
        self.declaration.icon = Some(code.into());
        self
    }

    pub fn item(
        &mut self,
        caption: impl Into<String>,
        configure: impl FnOnce(&mut NavigationItemBuilder),
    ) -> &mut Self {
        let mut builder = NavigationItemBuilder::new(caption);
        configure(&mut builder);
        self.declaration.items.push(builder.into_decl());
        self
    }
}
