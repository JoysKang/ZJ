; ZJ: minimal scopes and local bindings for in-file go to definition.
[(block) (method_declaration) (constructor_declaration) (lambda_expression) (for_statement) (enhanced_for_statement) (catch_clause)] @local.scope
(formal_parameter name: (identifier) @local.definition)
(local_variable_declaration declarator: (variable_declarator name: (identifier) @local.definition))
(enhanced_for_statement name: (identifier) @local.definition)
(lambda_expression parameters: (identifier) @local.definition)
(catch_formal_parameter name: (identifier) @local.definition)
