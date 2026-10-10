# Syntax check of every .jl file in the repository (no packages loaded, nothing executed).
# Usage: julia --project=tools tools/parsecheck.jl ; exits 1 if any file fails to parse.
bad(e) = e isa Expr && (e.head in (:error, :incomplete) || any(bad, e.args))
files = [joinpath(r, f) for (r, ds, fs) in walkdir(".") for f in fs
         if endswith(f, ".jl") && !occursin("/target/", r) && !occursin("/.git", r) && !occursin("/.claude/worktrees", r) && !occursin("/.remember", r)]
failed = filter(f -> bad(Meta.parseall(read(f, String); filename = f)), files)
println("julia files: ", length(files), "  parse failures: ", failed)
exit(isempty(failed) ? 0 : 1)
