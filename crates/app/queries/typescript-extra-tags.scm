; ZJ additions: type aliases, enums and class properties.
(type_alias_declaration name: (type_identifier) @name) @definition.type
(enum_declaration name: (identifier) @name) @definition.class
(public_field_definition name: (property_identifier) @name) @definition.field
