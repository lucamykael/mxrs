# `mxrs java-action`

```sh
mxrs java-action new <Module.Action> [--target DIR] [--dry-run] [--json]
```

A Java action is code a microflow runs on the server. Its contract — what a
call takes and returns — is the model's; its Java is the project's, and in
mxrs it runs the Rust registered for it. The scaffold writes both, as mxrb's
does:

- `src/ports/<module>/java_actions/<action>.rs` declares the action:

  ```rust
  #[declaration(module = "Sales")]
  pub fn invoke_checkout(module: &mut ModuleBuilder) {
      module.java_action("InvokeCheckout", |action| {
          action.documentation("Invoke checkout");
      });
  }
  ```

  Parameters and the return value are Rust types, as a JavaScript action's
  are: `action.takes::<MxString>("Value")`,
  `action.takes::<MxObject<Order>>("Order")`,
  `action.takes_optional::<MxInteger>("Limit")`,
  `action.returns::<MxList<Line>>()`; a type no Rust type names (an
  enumeration's) is `takes_type(name, CodeActionType::Enumeration(...))`.
  A build writes the `JavaActions$JavaAction` document Studio Pro stores for
  it, keeping the identities of one the model already has.

- `java/<module>/actions/<Action>.java` is the class Studio Pro writes for a
  new action, with its `BEGIN USER CODE` and `BEGIN EXTRA CODE` sections.
  Every build ships `java/` as the model's `javasource/`.

The importer writes the same declaration for every Java action of a module
the project made whose declaration states its stored document again; one
with a toolbox icon, a type parameter, a microflow parameter or a parameter
description stays in the imported model. Either way, the action's contract
for hand-written Rust is in its module's `ports::actions`, and the project
registers its implementation in `infrastructure::adapters::java_actions`. A
microflow calls the action with `call_java_action!(flow, "Sales.InvokeCheckout", ...)`.
The scaffold is transactional and refuses to overwrite either file.
