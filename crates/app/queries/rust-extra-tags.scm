; ZJ additions: constants, statics, fields and enum variants.
(const_item name: (identifier) @name) @definition.constant
(static_item name: (identifier) @name) @definition.constant
(field_declaration name: (field_identifier) @name) @definition.field
(enum_variant name: (identifier) @name) @definition.constant
(macro_invocation macro: (scoped_identifier name: (identifier) @name)) @reference.call
(call_expression function: (scoped_identifier name: (identifier) @name)) @reference.call
