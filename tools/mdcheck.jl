files = [joinpath(r,f) for (r,_,fs) in walkdir(".") for f in fs if endswith(f,".md") && !occursin("/target", r) && !occursin("/.git", r)]
odd = String[]; bad = Tuple{String,Int}[]
strip_code(l) = replace(replace(l, r"`[^`]*`" => ""), "\\|" => "")
for f in files
    L = readlines(f)
    count(l -> startswith(lstrip(l), "```"), L) % 2 == 1 && push!(odd, f)
    infence = false; i = 1
    while i <= length(L)
        l = L[i]
        if startswith(lstrip(l), "```"); infence = !infence; i += 1; continue; end
        if !infence && startswith(lstrip(l), "|") && i < length(L) && occursin(r"^\s*\|[\s:\-|]+\|\s*$", L[i+1])
            h = count(==('|'), strip_code(l)); j = i + 2
            while j <= length(L) && startswith(lstrip(L[j]), "|")
                count(==('|'), strip_code(L[j])) != h && push!(bad, (f, j)); j += 1
            end
            i = j
        else i += 1 end
    end
end
println("md files: ", length(files), "  odd fences: ", odd, "  table mismatches: ", bad)
exit(isempty(odd) && isempty(bad) ? 0 : 1)
