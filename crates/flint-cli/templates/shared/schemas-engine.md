The engine's components are the files in `{{engine}}/schemas/components` and its archetypes the files in `{{engine}}/schemas/archetypes`; a package's are in `{{engine}}/packages/<name>/schemas` (the walk's `player` is first-person's). There is no command that lists them: read the folder. `flint schema <name>` prints one, with its fields, types and defaults:

```bash
flint schema transform
flint schema character
```
