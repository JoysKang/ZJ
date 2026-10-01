; ZJ: minimal scopes and local bindings for in-file go to definition.
[(block) (function_declaration) (method_declaration) (func_literal) (for_statement) (if_statement)] @local.scope
(parameter_declaration name: (identifier) @local.definition)
(short_var_declaration left: (expression_list (identifier) @local.definition))
(var_spec name: (identifier) @local.definition)
(const_spec name: (identifier) @local.definition)
(range_clause left: (expression_list (identifier) @local.definition))
