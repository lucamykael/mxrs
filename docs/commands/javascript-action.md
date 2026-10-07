# `mxrs javascript-action`

```sh
mxrs javascript-action new <Module.Action> [--target DIR] [--dry-run] [--json]
```

A JavaScript action is code a nanoflow runs in the browser. Its contract —
what a call takes and returns, and where it runs — is the model's; its
JavaScript is the project's. The scaffold writes both, as mxrb's does:

- `src/ports/<module>/javascript_actions/<action>.rs` declares the action:

  ```rust
  #[declaration(module = "Sales")]
  pub fn open_map(module: &mut ModuleBuilder) {
      module.javascript_action("OpenMap", |action| {
          action
              .platform(JavaScriptPlatform::Web)
              .documentation("Open map");
      });
  }
  ```

  Parameters and the return value are Rust types:
  `action.takes::<MxString>("Address")`,
  `action.takes::<MxObject<Order>>("Order")`,
  `action.takes_optional::<MxInteger>("Zoom")`,
  `action.returns::<MxList<Pin>>()`; a type no Rust type names (an
  enumeration's) is `takes_type(name, CodeActionType::Enumeration(...))`.
  A build writes the `JavaScriptActions$JavaScriptAction` document Studio
  Pro stores for it, keeping the identities of one the model already has.

- `assets/javascriptsource/<module>/actions/<Action>.js` is the function
  Studio Pro writes for a new action, with its `BEGIN USER CODE` and
  `BEGIN EXTRA CODE` sections. Every build ships `assets/javascriptsource`
  beside the `.mpr`.

The importer writes the same file for every JavaScript action of a module the
project made whose declaration states its stored document again; one with a
toolbox icon, a type parameter, or a parameter description stays in the
imported model. The scaffold is transactional and refuses to overwrite
either file. A
nanoflow calls the action with `callJavaScriptAction("Sales.OpenMap", ...)`
in the frontend's TypeScript, or `call_javascript_action!` in Rust.
