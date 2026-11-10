; ZJ: minimal scopes and local bindings for in-file go to definition.
[(method) (singleton_method) (lambda) (block) (do_block) (class) (module)] @local.scope
(method_parameters (identifier) @local.definition)
(block_parameters (identifier) @local.definition)
(optional_parameter name: (identifier) @local.definition)
(keyword_parameter name: (identifier) @local.definition)
(splat_parameter name: (identifier) @local.definition)
(hash_splat_parameter name: (identifier) @local.definition)
(block_parameter name: (identifier) @local.definition)
(destructured_parameter (identifier) @local.definition)
(assignment left: (identifier) @local.definition)
(for (identifier) @local.definition)

; References
;-----------
(identifier) @local.reference
