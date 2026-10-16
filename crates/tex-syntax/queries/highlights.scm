; TeXoria highlight query for the latex-lsp tree-sitter grammar.
; Later captures override earlier ones, so broad patterns come first.

; ── Math ─────────────────────────────────────────────────────────────
(inline_formula) @embedded
(displayed_equation) @embedded
(math_environment) @embedded
(subscript) @embedded
(superscript) @embedded

; ── Generic commands ─────────────────────────────────────────────────
(command_name) @function
(placeholder) @variable.special
(operator) @operator
(delimiter) @punctuation.special

; ── Groups ───────────────────────────────────────────────────────────
["{" "}" "[" "]"] @punctuation.bracket

; ── Environments ─────────────────────────────────────────────────────
(begin command: _ @keyword name: (curly_group_text (text) @tag))
(end command: _ @keyword name: (curly_group_text (text) @tag))

; ── Sectioning ───────────────────────────────────────────────────────
(part command: _ @keyword text: (curly_group) @title)
(chapter command: _ @keyword text: (curly_group) @title)
(section command: _ @keyword text: (curly_group) @title)
(subsection command: _ @keyword text: (curly_group) @title)
(subsubsection command: _ @keyword text: (curly_group) @title)
(paragraph command: _ @keyword text: (curly_group) @title)
(subparagraph command: _ @keyword text: (curly_group) @title)
(title_declaration command: _ @keyword)
(author_declaration command: _ @keyword)
(caption command: _ @keyword)

; ── References ───────────────────────────────────────────────────────
(label_definition command: _ @keyword name: (curly_group_label (label) @label))
(label_reference command: _ @keyword names: (curly_group_label_list (label) @label))
(label_reference_range command: _ @keyword)
(citation command: _ @keyword keys: (curly_group_text_list (text) @string.special))
(hyperlink command: _ @keyword)
(curly_group_uri (uri) @link_uri)

; ── Includes & preamble ──────────────────────────────────────────────
(class_include command: _ @preproc path: (curly_group_path (path) @string))
(package_include command: _ @preproc paths: (curly_group_path_list (path) @string))
(latex_include command: _ @preproc path: (curly_group_path (path) @string))
(graphics_include command: _ @preproc path: (curly_group_path (path) @string))
(svg_include command: _ @preproc)
(inkscape_include command: _ @preproc)
(verbatim_include command: _ @preproc)
(import_include command: _ @preproc)
(bibtex_include command: _ @preproc paths: (curly_group_path_list (path) @string))
(bibstyle_include command: _ @preproc path: (curly_group_path (path) @string))
(biblatex_include) @preproc
(tikz_library_import command: _ @preproc)

; ── Definitions ──────────────────────────────────────────────────────
(new_command_definition command: _ @keyword declaration: (_) @type)
(old_command_definition command: _ @keyword declaration: (_) @type)
(let_command_definition command: _ @keyword)
(environment_definition command: _ @keyword name: (curly_group_text (text) @type))
(theorem_definition command: _ @keyword)
(paired_delimiter_definition command: _ @keyword)

(key_value_pair key: (_) @property)
(brack_group_argc) @number

; ── Verbatim & comments (last: they win over everything) ─────────────
(verbatim_environment verbatim: (_) @text.literal)
(listing_environment code: (_) @text.literal)
(minted_environment code: (_) @text.literal)
(line_comment) @comment
(block_comment) @comment
(comment_environment) @comment
(todo) @hint
