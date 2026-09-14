use mxrs_ir::{ExportLevel, MenuActionDecl, MenuDecl, MenuIconDecl, MenuItemDecl};

pub struct MenuBuilder {
    declaration: MenuDecl,
}

impl MenuBuilder {
    pub(crate) fn new(name: impl Into<String>) -> Self {
        Self {
            declaration: MenuDecl::new(name),
        }
    }

    pub(crate) fn into_decl(self) -> MenuDecl {
        self.declaration
    }

    pub fn documentation(&mut self, value: impl Into<String>) -> &mut Self {
        self.declaration.documentation = value.into();
        self
    }

    pub fn excluded(&mut self, value: bool) -> &mut Self {
        self.declaration.excluded = value;
        self
    }

    pub fn export_level(&mut self, value: ExportLevel) -> &mut Self {
        self.declaration.export_level = value;
        self
    }

    pub fn item(
        &mut self,
        caption: impl Into<String>,
        configure: impl FnOnce(&mut MenuItemBuilder),
    ) -> &mut Self {
        let mut builder = MenuItemBuilder::new(caption);
        configure(&mut builder);
        self.declaration.items.push(builder.into_decl());
        self
    }

    pub fn localized_item<I, K, V>(
        &mut self,
        captions: I,
        configure: impl FnOnce(&mut MenuItemBuilder),
    ) -> &mut Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let mut builder = MenuItemBuilder::new_localized(captions);
        configure(&mut builder);
        self.declaration.items.push(builder.into_decl());
        self
    }
}

pub struct MenuItemBuilder {
    declaration: MenuItemDecl,
}

impl MenuItemBuilder {
    fn new(caption: impl Into<String>) -> Self {
        let mut declaration = MenuItemDecl::default();
        declaration
            .caption
            .insert("en_US".to_string(), caption.into());
        Self { declaration }
    }

    fn new_localized<I, K, V>(captions: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let declaration = MenuItemDecl {
            caption: captions
                .into_iter()
                .map(|(locale, text)| (locale.into(), text.into()))
                .collect(),
            ..Default::default()
        };
        Self { declaration }
    }

    fn into_decl(self) -> MenuItemDecl {
        self.declaration
    }

    pub fn caption(&mut self, locale: impl Into<String>, text: impl Into<String>) -> &mut Self {
        self.declaration.caption.insert(locale.into(), text.into());
        self
    }

    pub fn alternative_text(
        &mut self,
        locale: impl Into<String>,
        text: impl Into<String>,
    ) -> &mut Self {
        self.declaration
            .alternative_text
            .get_or_insert_with(Default::default)
            .insert(locale.into(), text.into());
        self
    }

    pub fn no_action(&mut self) -> &mut Self {
        self.declaration.action = MenuActionDecl::default();
        self
    }

    pub fn page(&mut self, reference: impl Into<String>) -> &mut Self {
        self.declaration.action = MenuActionDecl::OpenPage {
            page: reference.into(),
            disabled_during_execution: true,
            pages_to_close: None,
            title_override: None,
        };
        self
    }

    pub fn create_object(
        &mut self,
        entity: impl Into<String>,
        page: impl Into<String>,
    ) -> &mut Self {
        self.declaration.action = MenuActionDecl::CreateObjectAndOpenPage {
            entity: entity.into(),
            page: page.into(),
            disabled_during_execution: true,
            pages_to_close: None,
            title_override: None,
        };
        self
    }

    pub fn action(&mut self, action: MenuActionDecl) -> &mut Self {
        self.declaration.action = action;
        self
    }

    pub fn glyph(&mut self, code: i64) -> &mut Self {
        self.declaration.icon = Some(MenuIconDecl::Glyph(code));
        self
    }

    pub fn image(&mut self, reference: impl Into<String>) -> &mut Self {
        self.declaration.icon = Some(MenuIconDecl::Image(reference.into()));
        self
    }

    pub fn item(
        &mut self,
        caption: impl Into<String>,
        configure: impl FnOnce(&mut MenuItemBuilder),
    ) -> &mut Self {
        let mut builder = MenuItemBuilder::new(caption);
        configure(&mut builder);
        self.declaration.items.push(builder.into_decl());
        self
    }

    pub fn localized_item<I, K, V>(
        &mut self,
        captions: I,
        configure: impl FnOnce(&mut MenuItemBuilder),
    ) -> &mut Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let mut builder = MenuItemBuilder::new_localized(captions);
        configure(&mut builder);
        self.declaration.items.push(builder.into_decl());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_localized_nested_menu_without_storage_details() {
        let mut menu = MenuBuilder::new("Main");
        menu.documentation("App navigation").item("Home", |item| {
            item.caption("pt_BR", "Início")
                .image("Atlas_Core.Atlas_Filled.home")
                .page("Sales.Home")
                .item("Child", |child| {
                    child.glyph(57_377);
                });
        });
        let declaration = menu.into_decl();
        assert_eq!(declaration.items[0].caption["pt_BR"], "Início");
        assert_eq!(declaration.items[0].items.len(), 1);
        assert!(matches!(
            declaration.items[0].action,
            MenuActionDecl::OpenPage { .. }
        ));
    }
}
