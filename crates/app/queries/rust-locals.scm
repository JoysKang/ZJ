; ZJ: minimal scopes and local bindings for in-file go to definition.
[(block) (function_item) (closure_expression) (for_expression) (match_arm)] @local.scope
(parameter pattern: (identifier) @local.definition)
(self_parameter) @local.definition
(let_declaration pattern: (identifier) @local.definition)
(let_condition pattern: (_ (identifier) @local.definition))
(closure_parameters (identifier) @local.definition)
(for_expression pattern: (identifier) @local.definition)
(tuple_pattern (identifier) @local.definition)
(tuple_struct_pattern type: (_) (identifier) @local.definition)
(match_pattern (identifier) @local.definition)
