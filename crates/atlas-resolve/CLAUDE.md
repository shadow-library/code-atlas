# atlas-resolve

Project-wide Rust and TypeScript name, module and import resolution. It turns per-file `FileFacts` plus each repo's `PackageGraph` into edges and `UnresolvedRef`s.

## Lifecycle

- `ProjectSymbolTable` keeps a compact surface per file: symbols, imports/exports, module declarations, impls, and derived type text. It never keeps references or bodies.
- The order is `set_repo_packages` → `upsert_file` / `remove_file` → `refresh()` → `resolve_file` / `resolve_path_in_file`. Resolving against a table with pending module-tree work returns `StaleTable`. Resolving facts whose content hash differs from the upserted surface returns `StaleFile`.
- `refresh()` recomputes the Rust module tree of dirty Cargo packages and returns `Relocation`s for files whose effective `KeyContext` changed. The engine must re-key those files, because keys built from `default_rust_module_path` are wrong for them. The relocations include files the engine did not touch.
- Resolving a file reads other files' surfaces only, never their references, so re-resolving a file needs only its own facts. The single exception is `item_macro_calls`, a flag for item-level macro invocations that is derived from references. It only changes unresolved reasons (`macro_generated` instead of `not_found`), never edges.

## Incremental dependency contract (for T12)

- `upsert_file` returns a `SurfaceDelta` and `remove_file` returns `Option<SurfaceDelta>`. A delta carries `change` (`Added`, `Removed`, `Changed` or `Unchanged`), the declaration `names` whose digest changed, and `broad`. A digest covers a name's symbols, impl members, use bindings, imports and exports, so edits to function bodies leave the surface `Unchanged`. A symbol's digest includes the type text a receiver chain reads from it: field types, return types, params, modifiers (`async`), generics and a type alias's target.
- Receiver chains read field types, return types and alias targets declared in other files. Every hop goes through a noted lookup (the field, method, type and alias names), so a changed field type, renamed field, changed return type, deleted or moved struct lists the files whose chains passed through it. The per-file memos (`return_cursors`, module lookups) never outlive one `resolve_file` call, so a memo hit never skips a note.
- `broad` is set when a file is added or removed, or when its module declarations, glob/`export *` statements, extern crates, item-macro flag or key context change.
- `resolve_file` records every name it looked up for the file, including lookups that ended in `NotFound`, and replaces that file's previous record. `resolve_path_in_file` (detector lookups) accumulates into a separate per-file set, which `resolve_file` never clears, so call order doesn't matter. `remove_file` forgets both.
- Name digests include the names a TS class extends or implements, and the self and trait names of a Rust impl. Adding `implements Repo` therefore lists the files that looked up `Repo`.
- `affected_files(deltas, relocations)` returns the other files to re-resolve:
  - every file whose recorded lookups mention a changed name;
  - for `broad` deltas and for relocations, every file of the same package and of packages that depend on it directly or transitively, or of the whole repo for unpackaged files. Transitive dependents are needed because dispatch candidates come from the linked package set (below), and whether a dependency's file is test code can change through another file's `mod` declaration.

  Changed files themselves are excluded, because the engine re-resolves them anyway.
- After `set_repo_packages`, the engine re-resolves the whole repo and its dependents. Manifests are not covered by deltas.

## Rust module tree

- The tree walks from every target root, in the order lib, bins, tests, examples, benches, build script. It follows `mod` declarations, `#[path]` and inline modules with rustc's mod-rs rules. Crate roots, `mod.rs` files and `#[path]` files are mod-rs, so `mod foo;` in `src/bin/x.rs` loads `src/bin/foo.rs`, not `src/bin/x/foo.rs`.
- `Reachability` values:
  - `TargetRoot` and `Reachable`.
  - `CfgGated { cfg }`: the file keeps its real module path and records the cfg text. The spec said to keep the default path here; the real path is used deliberately, because keys must match the paths that other files resolve to.
  - `Unreachable`: the file keeps its context and is indexed as a weak module location, used only when no reachable file provides that module.
  - `Unpackaged` and `NotApplicable` (TypeScript).
- A file reached from several crates gets one key context, the first by target order. The other crates are listed in `also_in`, and the file is indexed under all of them, so `tests/common/mod.rs` resolves from every test target.

## Certainty

- `syntactic` covers containment (file to top-level symbol, parent to child) and manifest `depends_on_package` edges.
- `resolved` covers unique results under emulated language rules. It also covers mod-tree containment, type to impl-method `contains`, and `implements`/`extends`.
- `heuristic` rules:
  - `dyn-dispatch-candidate` (0.3)
  - `interface-dispatch-candidate` (0.3)
  - `method-name-only`: 0.5 for one candidate, 0.35 for two, 0.25 for three
  - `dist-to-src` (0.85): weakens every hop resolved through it. It is the fallback when no tsconfig declares where a missing package entry was emitted from (below)
  - `macro-name-in-crate` (0.6)
  - `macro-token-scan` (0.6): caps every `MacroTokenScan` reference
  - `override-dispatch-candidate` (0.3): TS subclass overrides of a method resolved on an instance receiver (not through `super`), and Rust impls of a trait method called by path (`Self::m()`, `Trait::m(x)`)
  - `unwrap:Option` and `unwrap:Result` (0.8), `deref:MutexGuard`, `deref:RwLockReadGuard`, `deref:RwLockWriteGuard`, `deref:Ref` and `deref:RefMut` (0.8), and `constructor-signature` (0.9): receiver-chain hops (see below)
  - The three dispatch rules only consider implementers in the caller's linked package set: its own package, its direct in-project dependencies, and their normal (Cargo) or normal/peer (npm) dependencies, transitively. The direct dependencies a Rust caller sees depend on where it is: normal ones from lib or bin code, normal and dev ones from test code (`#[test]`, a `#[cfg(test)]` or `tests` module, a test path, or a test, bench or example target), build ones from a build script. TS callers see every section. An implementer in another package must also be library code: not on a test path (`is_test_path`), not in a `test`/`tests` module or a `#[cfg(test)]` file or inline module, and (Rust) in that package's lib target. So implementers in downstream packages are not candidates, although a trait object in library code may hold one at runtime; the declaration edge and the `implements` edges still lead there.
  - Exception: `self.m()` and `Self::m()` in a Rust trait's default method, and `this.m()` in a TS class, run as part of every implementer or subclass, so they consider all of them, wherever they live. Only test paths and in-file `test`/`tests` modules of other packages are excluded. An unpackaged caller considers its own repo with the same exclusions; files in another repo never count as its own package. File placement and file-level `#[cfg(test)]` are not consulted for these callers, because they are not downstream of the candidate's package, so `affected_files` does not re-resolve them when those change.
  - `param-type-recovery` (0.7): caps receiver types read from parameter text. That covers `Box`/`Arc`/`Rc` inner types behind a `TypeHint` (bindings in nested fn items), and generic bounds for an `Expr` receiver, which may be a shadowing local. A `TypeHint` or `Declared` receiver yields `resolved`: `Declared` resolves its own type text, with generics through the bounds of the enclosing fn and type, and wrappers through their argument.
  - `field-name-only` (0.5): a field access on an `Expr` or `Identifier` receiver of unknown type, when the field name matches exactly one Rust field visible from the caller (same package, or `pub` in an in-project dependency). Several matches are `Ambiguous`, and the public std field names `start`, `end`, `status`, `stdout` and `stderr` never match. A `self`, `TypeHint` or `Declared` receiver whose type does not resolve is `NotFound`, never a name match.
  - `fn-local-item-possible` (0.5): a type-like hit (struct, enum, variant, trait, alias or module) from a function whose body declares local `struct`/`enum`/`union`/`trait`/`type`/`mod` items. The analyzer emits no symbols for them. The same applies to `const`/`static` items, which carry a body fingerprint of their initializer: the item-keyword scan (`struct enum union trait type mod fn impl macro_rules!`) decides, with the span heuristic used only when there is no fingerprint. References inside the type annotation are exempt, because items declared in the initializer are not in scope there. The exemption ends at a boundary derived from the signature (`name end + normalised tail length`, a lower bound). It is disabled when the annotation itself contains a `{` block with an item keyword, and when the signature is truncated or the name is not found in it. A fingerprint truncated at `MAX_BODY_TOKENS` (`token_count` above the stored length) counts as possibly declaring items, for fns as well. Fn-scoped `use` targets are resolved with the fn's may-declare flag, so a head that binds a module-level module is weakened too. A name that a fn-local `impl` names is `Unsupported("fn-local item")`, and so is any name after a fn-scoped glob whose target can't be resolved (a fn-local enum or module). Such lookups never fall through to module scope.
- This crate never emits `compiler_verified`.
- A fn-scoped `use` or glob that binds a name differently from the enclosing module is `Ambiguous` with both candidates, because the analyzer reports block-scoped uses as fn-scoped.
- Generic parameters of the enclosing fn, impl, trait or struct (Rust), and type parameters of the enclosing class, method or function (TS), never resolve to same-named types. A generic with bounds becomes the bound traits; without bounds it is unknown.
- TS lookups are namespace-aware end to end (value vs type, through exports and re-exports). Instance receivers see only non-static members. `ClassName.m()`, `this` in static methods and `this` in static blocks (where `from` is the class symbol) see only static members. `Expr("this")` (local classes, object-literal methods, non-arrow functions) is `Unsupported("rebound this")`, with no candidates, when `from` lies inside a class, because the rebound receiver's members have no symbols and a name match could pick the class's same-named member. Outside a class it is an unknown receiver with name-only candidates. An overloaded function resolves to its implementation signature.
- Heuristic certainty is carried through every hop: re-exports, extends chains, field types, globs and dist→src.
- A package entry that does not exist is mapped to its source exactly (`resolved`) through the repo's tsconfigs. Every config that emits the entry's kind into a directory containing it maps it below its source root. A declaration entry (`.d.ts`, `.d.mts`, `.d.cts`) is emitted only under `declaration` or `composite`, into `declarationDir` when declared, else `outDir`; a script entry is emitted into `outDir` unless `emitDeclarationOnly`; a `noEmit` config emits neither. The source root is `rootDir`, else the config dir when `composite`, else the common base directory of `include` (TypeScript 5 computes it from the input files; the include bases are an upper bound of it). Emitted extensions map back in TypeScript's order (`.js`/`.d.ts` to `.ts`, `.tsx`, `.js`, `.jsx`; `.jsx` to `.tsx`, `.jsx`; `.mjs`/`.d.mts` to `.mts`, `.mjs`; `.cjs`/`.d.cts` to `.cts`, `.cjs`), and an extensionless entry or directory probes those script extensions, then `index`. A mapping whose source file does not exist means that config does not emit the entry. The hit is exact only when the remaining mappings name one file and no emitting config has an undeterminable source root; otherwise `dist-to-src` applies. `outDir`, `declarationDir`, `rootDir` and `include` resolve against the config that declares them, or against the extending config for `${configDir}`.
- Bundlers that flatten paths (tsup, vite) can make a declared `outDir`/`rootDir` mapping name a source file the bundle was not built from; the mapping is trusted because the compiler configuration declares it.
- Methods a deref wrapper provides itself (`clone`, `as_ref`, `strong_count`, …) are `External(std)` before the inner type is consulted.
- Bare `macro_rules!` names follow textual order within a file. Path-based macro lookup sees only `#[macro_export]` macros, except while resolving a `use`. A single-segment macro `use` (`pub(crate) use line;`) binds the `macro_rules!` textually in scope at the `use`, so a crate-local macro imported elsewhere shadows the std macro of the same name. The std macro prelude applies only when no project macro is found.
- A Cargo dependency is matched by path, or by name only in other repos. A name present in several other repos gives `Ambiguous`.
- An `Ambiguous` result whose candidates are all the same entity (for example cfg alternatives behind one `#[path]` module) is resolved. Name-only candidate lists are never collapsed.
- Glob and `export *` webs are searched breadth-first with a visited set, and completed top-level lookups are memoised per resolver run.
- A path with a `(expr)` head or an `[expr]` segment is `Dynamic` and gets no candidates. That includes method calls on an expression receiver, unless its receiver is a `ReceiverHint::Chain`.
- Receiver chains (`rust_chain.rs`) are walked through declared types only, and a chain that cannot be followed stays `Dynamic`, never a name-only guess:
  - the head is the binding's type (`self`, a `Declared` text, a `TypeHint`, a static's declared type) or a path call's declared return type. A path call whose segments before the function name a struct, enum, trait or alias (`T::new(..)`, `Self::new(..)`, `<T as Tr>::make(..)`) is weakened to `constructor-signature` (0.9); a free fn or module path call, a tuple-struct constructor and a variant constructor are not;
  - `Field` reads the field's declared type in the struct's module, with the struct's generics through their bounds as for plain field chains; `Method` reads the return type of the method `rust_method_or_std` picks (inherent, then trait impl, then trait default), and on a trait receiver the trait declaration's return type;
  - in return types, `Self` is the impl type, the concrete receiver for a trait default method, and unknown on a trait receiver; generic parameters of the fn, impl or trait are unknown unless bound by unification, and associated types (`Self::Out`, `T::Item`) are unknown;
  - a generic return type of a path call is bound by matching each param's declared type against the argument's declared type (outer path names and arity must agree, references are ignored); conflicting bindings are dropped;
  - `Box`/`Arc`/`Rc` are peeled as before; `clone` and `as_ref` on them keep the wrapped type, `as_mut` is unknown (only `Box` has `AsMut`), and `borrow`/`borrow_mut` follow the inner type's own method. They are unknown when the inner type has no such method, or when `Borrow`/`BorrowMut` may be in scope, because the wrapper's own impl then wins at the `&Rc` autoref step: any `use` in the file whose target is `std`/`core`/`alloc` `::borrow::Borrow`/`BorrowMut`, whatever it binds (`as _`, a rename), any glob of a `borrow` module, or any binding the caller's scope gives the name `Borrow`/`BorrowMut` (a project prelude glob re-exporting it, or an external glob, which may re-export it too). An external `Option<T>`/`Result<T, _>` followed by `?`, `unwrap`, `expect`, `unwrap_or`, `unwrap_or_else` or `unwrap_or_default` gives `T` with `unwrap:Option`/`unwrap:Result` (0.8); `as_ref`, `as_mut`, `cloned`, `copied`, `ok`, `ok_or` and `ok_or_else` keep the wrapper. `MutexGuard`, `RwLockReadGuard` and `RwLockWriteGuard` (std, parking_lot, tokio) and `Ref`/`RefMut` (std) deref to their first type argument with `deref:<Guard>` (0.8);
  - an `async fn` call is a future until `.await`; any other step on it, and `.await` on anything else (including `impl Future`), is unknown;
  - module-level type aliases are expanded with their arguments bound to the alias's generics; associated types in impls are not;
  - every hop's heuristic is carried to the edge, the weakest one wins, and the final call follows `rust_method_on_recv` (so a trait receiver yields the trait declaration plus `dyn-dispatch-candidate` edges). A wrapper left at the end (`Option`, `Result`, a future) makes the call `External`.
- An external glob (`use rayon::prelude::*;`) claims an otherwise unknown type-namespace name as `External` only when the name is not in the importing crate's extern prelude: a Cargo dependency of the file's package, or an `extern crate x as y;` alias at the crate root. The glob's contents cannot be enumerated, but those names never come from it.
- Unresolved refs store a chain receiver as `Expr` with the `(expr)` head's text, because they are display-only; artifacts keep the full chain for re-resolution.
- Rust `Expr("self")` receivers (fn, impl or trait items nested in a body) are `Unsupported("self in nested item")`, with no candidates: the nested item's members have no symbols, so a name match could pick the outer type's same-named member.
- Name-only candidates apply only when a named receiver's type is unknown. Candidates are limited to the caller's package and its in-project dependencies, excluding other crates' `#[cfg(test)]` code and impls of external traits. A ubiquitous std or JavaScript builtin method name (`iter`, `push`, `map`, `then`, and so on) becomes `Ambiguous` instead of a heuristic edge.

## Unresolved reasons and edge kinds

- `UnresolvedReason::Unsupported { detail: "macro_generated" }` marks names that project macros may generate; core has no dedicated variant. Other `Unsupported` details explain unsupported constructs.
- Dynamic `require(x)` and `import(x)` become `UnresolvedRef { kind: Call, path: ["require" | "import", <variable>] }`, because `RefKind` has no import kind.
- `External { package }` names:
  - the Cargo package name, or `std`, `core` or `alloc`
  - the npm package name
  - a Node builtin (`node:fs` or `fs`)
  - `global` for JavaScript globals
- Edge kind by reference kind:
  - `Call` and `MethodCall` map to `calls`; the kind is `instantiates` when the target is a struct, variant or class.
  - `Instantiation` maps to `instantiates`.
  - `TypeRef` maps to `type_of` from fields, constants and variables, and to `references` otherwise.
  - `ValueRef` and `MacroCall` map to `references`. A `ValueRef` with a receiver is a Rust field access: the chain is walked through declared field types and the last segment resolves to the owner's `field` symbol. It never gets a type anchor.
  - `Decorator` maps to `calls`.
  - `JsxElement` maps to `calls`, or `instantiates` for class components.
- `imports` and `re_exports` run from the file entity, or from the enclosing symbol for a dynamic `import()`. They live in `ResolvedFile.structural`.
- TS heritage clauses become `extends`/`implements` edges (class or interface to the resolved target). The analyzer also emits a `TypeRef` for each heritage path; when it is a `from` symbol's own `extends`/`implements` path and resolves to the same symbol as the heritage edge, it emits no `references` edge, so dependents see the heritage kind instead. Type arguments in the clause (`Store<User>`) keep their `references` edges, and an unresolved or external heritage path keeps its `UnresolvedRef`.
- Method-level overrides (TS), in `ResolvedFile.structural`: every `method` child of a class (methods, accessors, arrow-function properties) gets `implements` edges to the same-named `method` or `field` members of the nearest owner among each resolved `extends`/`implements` target and its resolved `extends` ancestors. Owners are searched breadth-first (at most `MAX_HERITAGE_DEPTH` symbols), and each target's ancestor chain is resolved once per class. The edge carries `via = "interface" | "class"` (`atlas_core::ATTR_OVERRIDE_VIA`), the kind of that owner. Certainty is `resolved`, weakened by every heritage hop (`dist-to-src`). Rules:
  - static methods match static members of classes only; interfaces have no static side;
  - every overload signature of the nearest owner is a target; when the owner has same-named members of the source's accessor kind (getter, setter or plain), only those are targets;
  - constructors, `private` and `#private` sources have no edge, and a nearest member that is private stops the search with no edge;
  - only class symbols are sources: an interface member that redeclares a super-interface member gets no edge;
  - heritage without a static path (mixin calls, conditional expressions), external targets and targets that are neither class nor interface give no edge.

## Known limits

- Fn-local items are invisible to the analyzer. A local type is detected only through a local `impl` (definite) or item keywords in the body (possible); a local type in a body truncated beyond `MAX_BODY_TOKENS` is not detected.
- `self`/`Self` inside an item nested in a body stay unresolved (`Unsupported("self in nested item")`, `Dynamic` for `(Self)` paths), even when the nested impl is for a module-level type.
- Member accesses on a rebound `this` inside a class stay unresolved, including functions invoked with the enclosing instance as `this` (`forEach(fn, this)`, `fn.call(this)`).
- Explicit `this: T` parameter annotations are ignored (vue `get(this: VueElement)`, pinia `$reset(this: …)`), so such receivers are rebound `this`.
- TS override edges come from `method` symbols only. A field whose value is not a function literal (hono `RegExpRouter.match: typeof match = match`) does not link to the interface member it implements.

- Rust `let x = T::new()` receivers stay unknown, because the analyzer emits `Expr` for untyped lets. Receiver chains do not infer either: `lock(&local)` with an untyped `local` stays `Dynamic`.
- A chain ending in a method that a project extension trait adds to `Option`/`Result` stays `External(std)`.
- Choosing between an inherent and a trait method ignores receiver mode (an inherent `fn m(self)` beats a trait `fn m(&self)` on `&T`), and receiver chains carry that choice into the next hop.
- An external glob still claims value-namespace names (`core_b::model::Inner.go()` with a unit struct as the head) when they are not found.
- Receiver chains do not cover field accesses after a call (`self.f().x` emits no `ValueRef`), index steps, closures called through fields, generic methods whose return type is a method generic (`collect::<T>()`), or std methods other than the listed unwrapping ones (`Vec::iter`, `HashMap::get` stay unknown).
- Bindings in fn items nested in a body keep the old hints, so their generic and wrapper receivers stay capped at `param-type-recovery`.
- Field accesses through a project `Deref` impl and tuple-struct fields (`.0`) are not resolved.
- Impl-level generic bounds are not available (`ImplFact.generic_params` has names only), so those generics are unknown receivers.
- When a deref wrapper's inner type lacks a method, the method is attributed to `std`.
- Methods of derived std or serde traits, and blanket std methods, on project types are `External`.
- A path call or value (`T::m()`, `<T as Trait>::m`, `T::CONST`, TS `Class.m`) whose method is external, unresolved or ambiguous still emits a `references` edge from the caller to `T`, with the type segment's certainty and the call's span, when `T` is a project struct, enum, trait, alias or class (TS: class or enum). Resolved project methods emit no extra edge. `Self`, generic params and `crate`/`self`/`super` heads never get one, and `MacroTokenScan` refs cap it at `macro-token-scan`.
