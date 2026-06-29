-- Remove end-of-chapter vignettes (images with class "spot") from EPUB output.
-- Applied only on the EPUB build (PANDOC_EPUB --lua-filter); the PDF/print
-- builds do not load this filter, so the vignettes stay in the PDF and KDP print.

local function has_spot(blk)
  local found = false
  pandoc.walk_block(blk, { Image = function(img)
    if img.classes:includes('spot') then found = true end
  end })
  return found
end

-- implicit_figures wraps a standalone image in a Figure
function Figure(fig)
  if has_spot(fig) then return {} end
end

-- fallback: a lone .spot image still in a paragraph
function Para(p)
  if #p.content == 1 and p.content[1].t == 'Image'
     and p.content[1].classes:includes('spot') then
    return {}
  end
end
