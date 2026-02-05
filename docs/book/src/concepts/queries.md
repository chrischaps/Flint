# Queries

> This page is a stub. Content coming soon.

Flint's query system provides a SQL-inspired language for filtering and inspecting entities. This page will cover:

- Query grammar (PEG, parsed by pest)
- Full operator reference: `==`, `!=`, `>`, `<`, `>=`, `<=`, `contains`
- Querying nested fields with dot notation (`door.locked`)
- Boolean logic: `and`, `or`, `not`
- How queries are used in constraint definitions
- Performance characteristics and limitations

Grammar overview:

```
entities where <condition>

<condition> := <field> <op> <value>
            | <condition> and <condition>
            | <condition> or <condition>
            | not <condition>
```
