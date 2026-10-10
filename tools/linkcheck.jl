# Checks that relative markdown links [text](path) point at existing files or directories.
# External links (http, https, mailto) and pure anchors are skipped. Exits 1 on a broken link.
files = [joinpath(r, f) for (r, _, fs) in walkdir(".") for f in fs
         if endswith(f, ".md") && !occursin("/target", r) && !occursin("/.git", r)]
broken = Tuple{String,String}[]
for f in files
    infence = false
    for l in eachline(f)
        startswith(lstrip(l), "```") && (infence = !infence; continue)
        infence && continue
        for m in eachmatch(r"\]\(([^)\s]+)\)", l)
            target = m.captures[1]
            occursin(r"^(https?:|mailto:|#)", target) && continue
            path = split(target, '#')[1]
            isempty(path) && continue
            ispath(normpath(joinpath(dirname(f), path))) || push!(broken, (f, target))
        end
    end
end
println("markdown files: ", length(files), "  broken links: ", broken)
exit(isempty(broken) ? 0 : 1)
