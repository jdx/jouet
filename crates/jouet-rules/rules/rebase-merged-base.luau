--!strict
-- The `rebase-merged-base` rule's script (rebase-merged-base.toml). The
-- matcher finds a rebase; the script asks whether the branch is a layer of
-- a stack whose prerequisite has merged.

-- The branch and its merged prerequisite, or nil.
local function merged_base(): (string?, string?)
	local layer = jouet.stack.layer()
	if layer then
		if layer.parent_merged then
			return layer.branch, layer.parent
		end
		return nil, nil
	end
	-- Without gh-stack's metadata, the PR's base on GitHub, when the
	-- network probes are on.
	local pr = jouet.pr.status()
	if pr and pr.state ~= "merged" and pr.state ~= "closed" then
		local base = jouet.pr.status(pr.base)
		if base and base.head == pr.base and base.state == "merged" then
			return pr.head, pr.base
		end
	end
	return nil, nil
end

local rule: JouetRule = {
	check = function(_call: JouetCall): JouetDecision?
		local branch, parent = merged_base()
		if not branch or not parent then
			return nil
		end
		return jouet.warn(
			`\`{branch}\` is stacked on \`{parent}\`, which has merged. Rebasing it replays commits the trunk already has in another form, conflicts, and needs a force push. Merge the trunk in instead: \`jouet stack sync\` does that from the local branch and checks the result (\`--dry-run\` first).`
		)
	end,
}

return rule
