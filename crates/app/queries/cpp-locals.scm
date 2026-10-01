; ZJ: minimal scopes and local bindings for in-file go to definition.
[(compound_statement) (function_definition) (for_statement)] @local.scope
(parameter_declaration declarator: (identifier) @local.definition)
(parameter_declaration declarator: (pointer_declarator declarator: (identifier) @local.definition))
(declaration declarator: (identifier) @local.definition)
(init_declarator declarator: (identifier) @local.definition)
(init_declarator declarator: (pointer_declarator declarator: (identifier) @local.definition))
