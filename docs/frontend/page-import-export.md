# Pages in and out: Markdown and Word

A page downloads as Markdown or as a Word document (.docx), and a page can be
made from either (AWD-53 criterion 9). Google Docs opens and saves .docx
(File > Download > Microsoft Word, and File > Open > Upload), so a .docx is
also the way to and from Google Docs; LibreOffice and Pages read and write it
too.

- Download: the page's **Download** menu, or
  `GET /api/v1/pages/{id}/export?format=markdown|docx`. Anyone who can see
  the page can download it.
- Import: **Import** on a project's pages, or
  `POST /api/v1/projects/{id}/pages/import?name=<file name>` with the file
  as the body (20 MiB at most). It needs the right to edit pages. The new
  page is named after the document's title, else the file's name; a title
  another page has gets a number, such as "Plan (2)". The answer's
  `setAside` names what the file held that a page can't keep.
- Agents write pages as Markdown with `page_write`; they don't need either
  route.

The code is `apps/server/src/page_docs/docx.rs` (the conversion) and
`apps/server/src/page_docs/transfer.rs` (the routes).

## Markdown

A Markdown download is the page's own Markdown under its title as a level-1
heading (`# Title`). Importing a Markdown file reads a leading `# Title` line
as the page's title and keeps the rest as it is.

## How each block maps to Word

| Page block | In Word (.docx) | Back from Word |
| --- | --- | --- |
| Title | A paragraph in the Title style, and the document's title property | The first Title paragraph, else the title property, becomes the page's name |
| Heading 1 to 6 | Heading 1 to Heading 6 styles | Heading styles by name (`heading 1` and on, in any language Word uses) or an outline level; Heading 7 to 9 become Heading 6 |
| Paragraph | A Normal paragraph | Any other paragraph |
| Bold, italic, strikethrough | Run bold, italic, strike | The same, also from the Strong and Emphasis character styles; double strikethrough reads as strikethrough |
| Inline code | The Inline Code character style (Consolas) | That style, any character style with "code" in its name, or a monospace font (Consolas, Courier New, Roboto Mono, Menlo and others) |
| Link | A hyperlink; a link to the app's own page is made absolute with the app's address | Hyperlinks and `HYPERLINK` fields; links back to the app become paths again. Links a page won't hold (other than http, https, mailto and the app's own paths) keep their text only |
| Line break | A line break | Line and carriage-return breaks; page and column breaks are dropped |
| Bullet list, nested | Word's bullets, one level per depth; an item's later paragraphs are indented under it | Any bulleted numbering, by level; an indented paragraph, code block or quote after an item stays in that item. **Fallback:** a heading, table or divider inside an item comes back after the list |
| Numbered list | Word's numbering, each list from its own start | Any counting format (1., a., i.) as a numbered list from its start |
| Checklist item | **Fallback:** ☐ or ☑ before the text, indented to its level (Word has no checklist) | A paragraph or list item starting with ☐, ☑ or ☒, or a list whose bullet is a box (Google Docs' checklists) |
| Code block | Paragraphs in the Code style, one per line | Consecutive Code-style (or Preformatted, Source Code) paragraphs, or paragraphs whose text is all in a monospace font, become one code block. **Fallback:** the language name isn't kept |
| Quote | Paragraphs in the Quote style | Quote, Intense Quote and Block Text paragraphs, gathered into one quote. **Fallback:** a list inside a quote comes back outside it |
| Divider | An empty paragraph with a bottom border | The same |
| Table | A table with a header row (repeats on each printed page), column alignment, merged cells | The first row (or any marked header row) is the header; alignment from each column's first row. **Fallback:** a page's table can't merge cells, so a merged cell keeps its text and the cells it covered are empty; several paragraphs in a cell join with spaces |
| Embed (a live view of tasks, a page, a board) | **Fallback:** a link to it, with its label | A link |

## What an import leaves out

Each of these is dropped, and named in `setAside` so the person is told:

- images and drawings (including text boxes and charts),
- footnotes and endnotes,
- comments,
- equations,
- embedded documents.

Tracked changes are read as accepted: insertions are kept and deletions left
out. Headers, footers, page layout, colours, fonts and sizes don't carry over.
