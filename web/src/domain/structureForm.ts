import type { MemoryStructure } from "../api/types";

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
  { id: "fact", label: "Fact", hint: "A durable truth about something or someone.", targets: ANY },
  { id: "experience", label: "Experience", hint: "Something that happened.", targets: ANY },
  { id: "learning", label: "Learning", hint: "The lesson drawn from something that happened.", targets: ANY },
  { id: "procedure", label: "Procedure", hint: "Steps to follow again.", targets: ANY },
  { id: "reference", label: "Reference", hint: "A pointer: a URL, a path, a ticket.", targets: ANY },
  { id: "goal", label: "Goal", hint: "Something you are working towards.", targets: ["me", "project"] },
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
}

export type StructureErrors = Partial<Record<keyof StructureValues, string>>;

export function emptyStructure(): StructureValues {
  return { kind: "", topic: "", target: "", project: "" };
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
  };
}
