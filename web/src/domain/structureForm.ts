import type { Memory, MemoryStructure, StructurePatch } from "../api/types";

// Mirrors src/memories/domain/kind.rs and topic.rs, so the form can offer only
// what the daemon will accept and explain it before the round trip. The daemon
// stays the authority (ai-specs/memory-structure-plan.md §3–§4).

export type Target = "me" | "project" | "general";

export interface KindOption {
  id: string;
  label: string;
  /** The question that decides it. */
  hint: string;
  targets: readonly Target[];
}

const ANY: readonly Target[] = ["me", "project", "general"];

export const KINDS: readonly KindOption[] = [
  { id: "preference", label: "Preference", hint: "What you pick when free to choose.", targets: ["me"] },
  { id: "rule", label: "Rule", hint: "A hard line, never to be crossed.", targets: ANY },
  { id: "convention", label: "Convention", hint: "How one project agreed to do things.", targets: ["project"] },
  { id: "decision", label: "Decision", hint: "A choice made at a point in time, for a reason.", targets: ["project", "general"] },
  { id: "fact", label: "Fact", hint: "A durable truth about something or someone. About you, it is your personal information.", targets: ANY },
  { id: "experience", label: "Experience", hint: "Something that happened.", targets: ANY },
  { id: "learning", label: "Learning", hint: "The lesson drawn from something that happened.", targets: ANY },
  { id: "procedure", label: "Procedure", hint: "Steps to follow again.", targets: ANY },
  { id: "reference", label: "Reference", hint: "A pointer: a URL, a path, a ticket.", targets: ANY },
  { id: "goal", label: "Goal", hint: "Something you are working towards. It stays open until you close it.", targets: ["me", "project"] },
  { id: "plan", label: "Plan", hint: "A dated thing ahead. It needs an end date, and leaves recall after it.", targets: ["project", "general"] },
];

export const TOPICS: readonly { id: string; label: string }[] = [
  { id: "coding", label: "Coding" },
  { id: "tech", label: "Tech (servers, services, setup)" },
  { id: "work", label: "Work" },
  { id: "family", label: "Family" },
  { id: "people", label: "People" },
  { id: "health", label: "Health" },
  { id: "lifestyle", label: "Lifestyle" },
  { id: "home", label: "Home" },
  { id: "finance", label: "Finance" },
  { id: "travel", label: "Travel" },
  { id: "food", label: "Food" },
];

export const TARGETS: readonly { id: Target; label: string; hint: string }[] = [
  { id: "me", label: "Me, everywhere", hint: "Who you are and how you like things, in every project." },
  { id: "project", label: "One project", hint: "True of one project only." },
  { id: "general", label: "General", hint: "Your world outside any project: machines, files, places." },
];

export interface StructureValues {
  kind: string;
  topic: string;
  target: Target | "";
  project: string;
  /** `YYYY-MM-DD`, or "" for none. Required for a plan, not allowed on a goal. */
  validUntil: string;
}

export type StructureErrors = Partial<Record<keyof StructureValues, string>>;

export function emptyStructure(): StructureValues {
  return { kind: "", topic: "", target: "", project: "", validUntil: "" };
}

export function kindOf(id: string): KindOption | undefined {
  return KINDS.find((kind) => kind.id === id);
}

export function validateStructure(values: StructureValues): StructureErrors {
  const errors: StructureErrors = {};
  if (values.kind === "") errors.kind = "Choose what sort of memory this is.";
  if (values.topic === "") errors.topic = "Choose what it is about.";
  if (values.target === "") {
    errors.target = "Choose who or what it is true for.";
  } else {
    const kind = kindOf(values.kind);
    if (kind !== undefined && !kind.targets.includes(values.target)) {
      errors.target = `A ${kind.label.toLowerCase()} applies to ${kind.targets
        .map((id) => TARGETS.find((t) => t.id === id)?.label.toLowerCase())
        .join(" or ")}.`;
    }
  }
  if (values.target === "project" && values.project.trim() === "") errors.project = "Name the project.";
  if (values.kind === "plan" && values.validUntil === "") errors.validUntil = "A plan needs the date it is over.";
  if (values.kind === "goal" && values.validUntil !== "") {
    errors.validUntil = "A goal has no end date: it stays open until you close it.";
  }
  return errors;
}

/**
 * The structure to send. Typed here by the user, so a project they name is
 * one they mean: a name the daemon does not know yet is created
 * (`new_project`), where an agent would have to ask first.
 */
export function structureOf(values: StructureValues): MemoryStructure {
  const target = values.target === "" ? "general" : values.target;
  return {
    kind: values.kind,
    topic: values.topic,
    applies_to:
      target === "project"
        ? { target, project_name: values.project.trim().toLowerCase(), new_project: true }
        : { target },
    origin: "user_said",
    ...(values.validUntil !== "" && { valid_until: endOfDay(values.validUntil) }),
  };
}

/** A date as the end of that day, UTC: "through the 14th" (as the MCP tools read it). */
export function endOfDay(date: string): string {
  return `${date}T23:59:59Z`;
}

/** A stored end as the date input shows it. */
export function dateOf(validUntil: string | null): string {
  return validUntil === null ? "" : validUntil.slice(0, 10);
}

/** What a stored memory is, as the form edits it. */
export function structureValuesOf(memory: Memory): StructureValues {
  return {
    kind: memory.kind,
    topic: memory.topic,
    target: memory.applies_to.target,
    project: memory.applies_to.project_name ?? "",
    validUntil: dateOf(memory.valid_until),
  };
}

/** Only what changed between `memory` and `values`, as a PATCH; `null` when nothing did. */
export function structurePatchOf(memory: Memory, values: StructureValues): StructurePatch | null {
  const patch: StructurePatch = {};
  if (values.kind !== memory.kind) patch.kind = values.kind;
  if (values.topic !== memory.topic) patch.topic = values.topic;
  const project = values.project.trim().toLowerCase();
  if (
    values.target !== "" &&
    (values.target !== memory.applies_to.target ||
      (values.target === "project" && project !== memory.applies_to.project_name))
  ) {
    patch.applies_to =
      values.target === "project" ? { target: "project", project_name: project, new_project: true } : { target: values.target };
  }
  if (values.validUntil !== dateOf(memory.valid_until)) {
    patch.valid_until = values.validUntil === "" ? null : endOfDay(values.validUntil);
  }
  if (Object.keys(patch).length === 0) return null;
  // The user is the one restating it, here.
  return { ...patch, origin: "user_said" };
}

/** A memory's structure in words: "Convention · Coding · quill". */
/**
 * A fact about the user is their personal information (who they are, their
 * skills, their work): its own group in the list, though the daemon stores it
 * as kind `fact` with target `me`.
 */
export const PERSONAL_INFORMATION = "personal";

/** The group a memory is listed under: its kind, or personal information. */
export function kindKeyOf(memory: Pick<Memory, "kind" | "applies_to">): string {
  return memory.kind === "fact" && memory.applies_to.target === "me" ? PERSONAL_INFORMATION : memory.kind;
}

/** A group's label: "Personal information", or the kind's. */
export function kindKeyLabel(key: string): string {
  return key === PERSONAL_INFORMATION ? "Personal information" : (kindOf(key)?.label ?? key);
}

export function describeStructure(memory: Memory, topicLabel: (id: string) => string = topicLabelOf): string {
  const kind = kindKeyLabel(kindKeyOf(memory));
  const scope =
    memory.applies_to.target === "project"
      ? (memory.applies_to.project_name ?? "a project")
      : memory.applies_to.target === "me"
        ? "you"
        : "general";
  return `${kind} · ${topicLabel(memory.topic)} · ${scope}`;
}

/** A built-in topic's default label; a user's own topic, from its id. */
export function topicLabelOf(id: string): string {
  const builtIn = TOPICS.find((t) => t.id === id);
  if (builtIn !== undefined) return builtIn.label.replace(/ \(.*\)$/, "");
  const words = id.replace(/[-_]/g, " ");
  return words === "" ? "Topic" : words[0]!.toUpperCase() + words.slice(1);
}
