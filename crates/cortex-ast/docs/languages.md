# Language Support

CortexAST ships with 3 statically linked **core languages** in this branch and recognizes 8 additional non-core language names for bundled Wasm grammars.

## Core Languages (always active)

| Language   | Extensions       | Parser  |
|------------|-----------------|---------|
| Rust       | `.rs`           | Static  |
| TypeScript | `.ts`, `.tsx`   | Static  |
| Python     | `.py`           | Static  |

## Non-Core Wasm Languages

Use `cortex_manage_ast_languages` with `action=status` to inspect the active/core language set plus the known non-core language names. In this minimal branch, `action=add` is intentionally unsupported: the runtime does not download grammars from the network and expects any extra `.wasm` files to be bundled locally.

| Language | Extensions                            | Known name |
|----------|---------------------------------------|------------|
| Go       | `.go`                                 | `go`       |
| PHP      | `.php`, `.php5`, `.phtml`             | `php`      |
| C++      | `.cpp`, `.cc`, `.cxx`, `.hpp`, `.hxx` | `cpp`      |
| C        | `.c`, `.h`                            | `c`        |
| C#       | `.cs`                                 | `c_sharp`  |
| Java     | `.java`                               | `java`     |
| Ruby     | `.rb`, `.rake`                        | `ruby`     |
| Dart     | `.dart`                               | `dart`     |

### Example

```json
{
  "action": "status"
}
```

If you bundle extra grammar files locally, place them where the runtime expects cached grammars and then restart or hot-reload the MCP worker.
Prune queries are bundled in the binary and written into the grammar cache directory on startup.
