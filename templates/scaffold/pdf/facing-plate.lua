-- Facing-page illustration plates, PDF/LaTeX only.
-- A chapter that opens with a standalone image becomes a FULL-PAGE plate on the
-- facing (verso/left) page, and the chapter heading + body open together on the
-- recto (right) across from it. The image is moved BEFORE the heading so the
-- title is not stranded on its own page. EPUB/HTML are untouched (image inline).
if FORMAT:match('latex') then
  -- Full-page plate forced onto a verso page; the next \chapter then lands on the
  -- facing recto (it is already a fresh recto, so \cleardoublepage adds no blank).
  local function plate(src)
    return pandoc.RawBlock('latex', table.concat({
      '\\cleartoverso',
      '\\thispagestyle{empty}',
      '\\AddThisPageImage{' .. src .. '}',
      '\\null\\clearpage',
    }, '\n'))
  end

  -- Small end-of-chapter spot illustration (tailpiece): centered, not full-page.
  function Para(el)
    if #el.content == 1 and el.content[1].t == 'Image'
       and el.content[1].classes:includes('spot') then
      local src = el.content[1].src
      return pandoc.RawBlock('latex', table.concat({
        '\\par\\vspace{1.5em}\\begin{center}',
        '\\includegraphics[width=2.4in,height=2.4in,keepaspectratio]{' .. src .. '}',
        '\\end{center}\\vspace{1em}',
      }, '\n'))
    end
  end

  -- Reorder: a chapter-opening image becomes a full-page verso plate emitted
  -- BEFORE its heading, so heading + body flow together on the facing recto.
  function Pandoc(doc)
    local out = {}
    local blocks = doc.blocks
    local i = 1
    while i <= #blocks do
      local b  = blocks[i]
      local nb = blocks[i + 1]
      if b.t == 'Header' and b.level == 1 and nb and nb.t == 'Para'
         and #nb.content == 1 and nb.content[1].t == 'Image'
         and not nb.content[1].classes:includes('spot') then
        table.insert(out, plate(nb.content[1].src))  -- image on the verso
        table.insert(out, b)                         -- heading on the facing recto
        i = i + 2                                     -- consume header + image
      else
        table.insert(out, b)
        i = i + 1
      end
    end
    return pandoc.Pandoc(out, doc.meta)
  end
end
