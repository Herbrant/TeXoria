; TeXoria highlight query for the latex-lsp tree-sitter BibTeX grammar.
; Only capture names defined by the themes, with the colors of the LaTeX query.

[
  (entry_type)
  (string_type)
  (preamble_type)
] @keyword

[
  (key_brace)
  (key_paren)
] @string.special

(field name: (identifier) @property)

[
  (brace_word)
  (quote_word)
] @string

(number) @number

; `@string` abbreviations, where they are defined and used.
(string name: (identifier) @variable.special)
(token (identifier) @variable.special)

(command) @function

[
  (comment)
  (junk)
] @comment

["=" "#"] @operator

["{" "}" "(" ")"] @punctuation.bracket

"," @punctuation.delimiter
