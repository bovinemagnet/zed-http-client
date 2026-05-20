(request_separator) @punctuation.special
(request_name) @title
(method) @keyword
(url) @string.special.url
(comment) @comment
(variable_declaration
  name: (identifier) @variable)
(variable_declaration
  value: (variable_value) @string)
(interpolation) @variable.special
(header_name) @property
(header_value) @string
(json_string) @string
(json_number) @number
