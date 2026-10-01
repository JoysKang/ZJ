; ZJ additions: enums, records, constructors and fields.
(enum_declaration name: (identifier) @name) @definition.class
(record_declaration name: (identifier) @name) @definition.class
(constructor_declaration name: (identifier) @name) @definition.method
(field_declaration declarator: (variable_declarator name: (identifier) @name)) @definition.field
