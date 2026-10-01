; ZJ: minimal scopes and local bindings for in-file go to definition.
[(function_definition) (lambda) (list_comprehension) (dictionary_comprehension) (generator_expression)] @local.scope
(parameters (identifier) @local.definition)
(default_parameter name: (identifier) @local.definition)
(typed_parameter (identifier) @local.definition)
(typed_default_parameter name: (identifier) @local.definition)
(assignment left: (identifier) @local.definition)
(for_statement left: (identifier) @local.definition)
(for_in_clause left: (identifier) @local.definition)
