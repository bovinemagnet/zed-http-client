module.exports = grammar({
  name: "http_request",

  extras: () => [/\r/, /[ \t]+/],

  rules: {
    source_file: ($) => repeat(choice($.comment, $.variable_declaration, $.request, $.blank_line)),

    blank_line: () => /\n+/, 

    comment: () => token(choice(seq("#", /.*/), seq("//", /.*/))),

    variable_declaration: ($) => seq("@", field("name", $.identifier), optional(/[ \t]*/), "=", optional(/[ \t]*/), field("value", $.variable_value), "\n"),

    identifier: () => /[A-Za-z_][A-Za-z0-9_.-]*/,
    variable_value: () => /[^\n]*/,

    request: ($) => seq(
      $.request_separator,
      optional(field("name", $.request_name)),
      "\n",
      repeat(choice($.comment, $.blank_line)),
      $.request_line,
      "\n",
      repeat(seq($.header, "\n")),
      optional(seq("\n", field("body", $.body)))
    ),

    request_separator: () => "###",
    request_name: () => /[^\n]+/,

    request_line: ($) => seq(field("method", $.method), /[ \t]+/, field("url", $.url)),

    method: () => choice("GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "GRAPHQL"),
    url: ($) => repeat1(choice($.interpolation, /[^\s\n]+/)),
    interpolation: () => /\{\{[^}\n]+\}\}/,

    header: ($) => seq(field("name", $.header_name), ":", optional(/[ \t]*/), field("value", $.header_value)),
    header_name: () => /[A-Za-z0-9-]+/,
    header_value: ($) => repeat1(choice($.interpolation, /[^\n]+/)),

    body: ($) => repeat1(choice($.json_string, $.json_number, $.interpolation, /[^\n]+/, "\n")),
    json_string: () => /"([^"\\]|\\.)*"/,
    json_number: () => /-?\d+(\.\d+)?/
  }
});
