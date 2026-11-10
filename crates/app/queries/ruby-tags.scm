; Definitions. `class A::B` defines B; the enclosing constant is only a reference.
(class
  name: (constant) @name) @definition.class

(class
  name: (scope_resolution
    name: (constant) @name)) @definition.class

(module
  name: (constant) @name) @definition.module

(module
  name: (scope_resolution
    name: (constant) @name)) @definition.module

(method
  name: (identifier) @name) @definition.method

(singleton_method
  name: (identifier) @name) @definition.method

(assignment
  left: (constant) @name) @definition.constant

(assignment
  left: (scope_resolution
    name: (constant) @name)) @definition.constant

; References
;-----------
(call
  method: (identifier) @name) @reference.call

(call
  receiver: (constant) @name) @reference.call

(call
  receiver: (scope_resolution
    name: (constant) @name)) @reference.call
