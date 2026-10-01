; ZJ additions: macros and enum constants.
(preproc_def name: (identifier) @name) @definition.constant
(preproc_function_def name: (identifier) @name) @definition.macro
(enumerator name: (identifier) @name) @definition.constant
(call_expression function: (identifier) @name) @reference.call
(namespace_definition name: (namespace_identifier) @name) @definition.module
