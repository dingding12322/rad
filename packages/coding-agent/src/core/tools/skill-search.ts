import type { AgentTool } from "@earendil-works/pi-agent-core";
import { type Static, Type } from "typebox";
import { Bm25Ranker, tokenize } from "../../extensions/tool-search/tool.ts";
import type { ToolDefinition } from "../extensions/types.ts";
import { formatSkillsForPrompt, type Skill } from "../skills.ts";
import { wrapToolDefinition } from "./tool-definition-wrapper.ts";

const skillSearchSchema = Type.Object({
	query: Type.String({ description: "Search terms for skill names and descriptions.", minLength: 1 }),
});

export type SkillSearchToolInput = Static<typeof skillSearchSchema>;

export interface SkillSearchToolDetails {
	skills: Pick<Skill, "name" | "description" | "filePath">[];
}

export interface SkillSearchToolOptions {
	/** Current resource-loader catalog. Without it, searches return no matches. */
	getSkills?: () => readonly Skill[];
}

export function createSkillSearchToolDefinition(
	options: SkillSearchToolOptions = {},
): ToolDefinition<typeof skillSearchSchema, SkillSearchToolDetails> {
	const segmenter = new Intl.Segmenter("en", { granularity: "word" });
	const ranker = new Bm25Ranker({
		tokenizer: (text) => [
			...tokenize(text),
			...Array.from(segmenter.segment(text.toLowerCase()))
				.filter(({ segment, isWordLike }) => isWordLike && /[^\x00-\x7f]/.test(segment))
				.map(({ segment }) => segment),
		],
	});
	return {
		name: "skill_search",
		label: "skill_search",
		description:
			"Search available skills by name and description. Returns at most 8 matching names, descriptions, and file paths. Read a matching skill file to load its instructions; resolve its relative references against the skill's directory.",
		promptSnippet: "Find skills by name and description; read matching files for instructions",
		parameters: skillSearchSchema,
		async execute(_toolCallId, { query }) {
			if (!query.trim()) throw new Error("query must not be empty");
			const candidates = (options.getSkills?.() ?? []).filter((skill) => !skill.disableModelInvocation);
			const matches = ranker.rank(
				query,
				candidates.map((skill) => ({ name: skill.filePath, text: `${skill.name} ${skill.description}` })),
				8,
			);
			const skillsByPath = new Map(candidates.map((skill) => [skill.filePath, skill]));
			const skills = matches.flatMap((match) => {
				const skill = skillsByPath.get(match.name);
				if (!skill) return [];
				const description =
					skill.description.length > 512 ? `${skill.description.slice(0, 509)}...` : skill.description;
				return [{ ...skill, description }];
			});
			return {
				content: [
					{ type: "text", text: formatSkillsForPrompt(skills, "indirect").trim() || "No matching skills found." },
				],
				details: { skills: skills.map(({ name, description, filePath }) => ({ name, description, filePath })) },
			};
		},
	};
}

export function createSkillSearchTool(options?: SkillSearchToolOptions): AgentTool<typeof skillSearchSchema> {
	return wrapToolDefinition(createSkillSearchToolDefinition(options));
}
