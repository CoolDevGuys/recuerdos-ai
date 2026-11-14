import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useId, useState, type FormEvent, type ReactNode } from "react";
import type { ApiClient } from "../api/client";
import type { About, DaemonDetails, SelfNames } from "../api/types";
import { memoriesCount } from "../domain/categories";
import { memoriesQueryKey } from "../state/memories";
import { useToasts } from "../state/toasts";
import { field, hint, label, primaryButton } from "./styles";

const dateTime = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });

const card = "rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-800 dark:bg-slate-900";
const heading = "text-sm font-semibold text-slate-900 dark:text-slate-100";
const term = "text-sm text-slate-600 dark:text-slate-400";
const value = "text-sm text-slate-900 dark:text-slate-100";
const code = "font-mono text-[13px] break-all";

export function aboutQueryKey(user: string) {
  return ["about", user] as const;
}

/**
 * What this daemon is: its version and build, who the key belongs to, and —
 * for an admin key, or with auth off — how it was started: the config file,
 * the `RECUERDOS_AI_*` overrides and the settings in effect. The same report
 * as `recuerdos-ai config`; secrets are never part of it.
 */
export function selfNamesQueryKey(user: string) {
  return ["self-names", user] as const;
}

export function AboutView({ client, user, canWrite }: { client: ApiClient; user: string; canWrite: boolean }) {
  const about = useQuery<About, Error>({ queryKey: aboutQueryKey(user), queryFn: () => client.about() });
  if (about.isPending) return <p role="status" className="text-sm text-slate-600 dark:text-slate-400">Loading…</p>;
  if (about.isError) {
    return (
      <p role="alert" className="text-sm text-red-700 dark:text-red-400">
        The daemon's details could not be loaded: {about.error.message}
      </p>
    );
  }
  const info = about.data;
  return (
    <section aria-label="About" className="space-y-4">
      <You client={client} user={user} canWrite={canWrite} />

      <div className={card}>
        <h2 className={heading}>Recuerdos AI {info.version}</h2>
        <Rows>
          <Row name="Build">
            <span className={code}>{info.git_sha || "unknown"}</span>
          </Row>
          <Row name="Running since">{formatDate(info.started_at)}</Row>
          <Row name="Language model">{info.understanding ? "Configured" : "None (saving and recall work; extraction and Suggest do not)"}</Row>
        </Rows>
      </div>

      <div className={card}>
        <h2 className={heading}>This key</h2>
        <Rows>
          <Row name="Signed in as">{info.user}</Row>
          <Row name="Scopes">{info.scopes.join(", ") || "none"}</Row>
          <Row name="Authentication">{info.auth_mode === "none" ? "Off: every request is the one user" : "API keys"}</Row>
        </Rows>
      </div>

      {info.daemon === null ? (
        <p className="text-sm text-slate-600 dark:text-slate-400">
          The configuration file, overrides and settings are shown to an admin key. Run{" "}
          <code className={code}>recuerdos-ai config</code> on the server to see them there.
        </p>
      ) : (
        <Configuration daemon={info.daemon} />
      )}
    </section>
  );
}

/**
 * Who "the user" is. Agents write about the user as "the user"; some name
 * them. With the names, the daemon knows both mean the same person: it files
 * a memory naming them as theirs, says so in the profile, and on saving sends
 * the memories filed as someone else's to Needs review.
 */
function You({ client, user, canWrite }: { client: ApiClient; user: string; canWrite: boolean }) {
  const id = useId();
  const queryClient = useQueryClient();
  const { show } = useToasts();
  const names = useQuery<SelfNames, Error>({ queryKey: selfNamesQueryKey(user), queryFn: () => client.selfNames() });
  const [draft, setDraft] = useState<{ full: string; others: string } | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  if (names.isPending) return null;
  if (names.isError) {
    return (
      <p role="alert" className="text-sm text-red-700 dark:text-red-400">
        Your names could not be loaded: {names.error.message}
      </p>
    );
  }
  const [first = "", ...rest] = names.data.names;
  const values = draft ?? { full: first, others: rest.join(", ") };

  const save = async (event: FormEvent) => {
    event.preventDefault();
    const list = [values.full, ...values.others.split(",")].map((n) => n.trim()).filter((n) => n !== "");
    setBusy(true);
    setFailure(null);
    try {
      const saved = await client.setSelfNames(list);
      queryClient.setQueryData(selfNamesQueryKey(user), saved);
      await queryClient.invalidateQueries({ queryKey: memoriesQueryKey(user) });
      setDraft(null);
      const flagged = saved.flagged ?? 0;
      show({
        tone: "success",
        message:
          flagged > 0
            ? `Saved. ${memoriesCount(flagged)} about you ${flagged === 1 ? "was" : "were"} filed as someone else's: see Needs review.`
            : "Saved your names.",
      });
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form className={card} onSubmit={(event) => void save(event)} aria-labelledby={`${id}-title`}>
      <h2 id={`${id}-title`} className={heading}>
        You
      </h2>
      <p className={hint}>
        Agents write about you as “the user”, and some use your name. With your names here, both are known to be you:
        a memory that names you is filed as your personal information, and the profile tells every agent who “the user”
        is.
      </p>
      <div className="mt-3 grid gap-3 sm:grid-cols-2">
        <div>
          <label htmlFor={`${id}-full`} className={label}>
            Your name
          </label>
          <input
            id={`${id}-full`}
            className={field}
            value={values.full}
            disabled={!canWrite || busy}
            placeholder="Alejandro Romero"
            onChange={(event) => setDraft({ ...values, full: event.target.value })}
          />
        </div>
        <div>
          <label htmlFor={`${id}-others`} className={label}>
            Other names you go by
          </label>
          <input
            id={`${id}-others`}
            className={field}
            value={values.others}
            disabled={!canWrite || busy}
            placeholder="Alejandro, Alex"
            aria-describedby={`${id}-others-hint`}
            onChange={(event) => setDraft({ ...values, others: event.target.value })}
          />
          <p id={`${id}-others-hint`} className={hint}>
            Separated by commas.
          </p>
        </div>
      </div>
      {failure !== null && (
        <p role="alert" className="mt-2 text-sm text-red-700 dark:text-red-400">
          {failure}
        </p>
      )}
      {canWrite && (
        <div className="mt-3">
          <button type="submit" className={primaryButton} disabled={busy || draft === null}>
            {busy ? "Saving…" : "Save names"}
          </button>
        </div>
      )}
    </form>
  );
}

function Configuration({ daemon }: { daemon: DaemonDetails }) {
  const file = daemon.config_file;
  return (
    <>
      <div className={card}>
        <h2 className={heading}>How it was started</h2>
        <Rows>
          <Row name="Command">
            <code className={code}>recuerdos-ai {daemon.command.join(" ")}</code>
          </Row>
          <Row name="Config file">
            {file.path === null ? (
              "None: built-in defaults and RECUERDOS_AI_* variables only"
            ) : (
              <>
                <code className={code}>{file.path}</code>
                <span className="block text-xs text-slate-500 dark:text-slate-400">
                  {file.from === "flag" ? "From --config" : "From RECUERDOS_AI_CONFIG"}
                  {file.exists ? "" : " · the file does not exist, so defaults apply"}
                </span>
              </>
            )}
          </Row>
          <Row name="Overrides">
            {daemon.overrides.length === 0 ? (
              "None"
            ) : (
              <ul aria-label="Overrides" className="space-y-0.5">
                {daemon.overrides.map((name) => (
                  <li key={name} className={code}>
                    {name}
                  </li>
                ))}
              </ul>
            )}
          </Row>
        </Rows>
        <p className="mt-3 text-xs text-slate-500 dark:text-slate-400">
          Overrides are listed by name only. Settings from them win over the config file.
        </p>
      </div>

      <div className={card}>
        <h2 className={heading}>Settings in effect</h2>
        <div className="mt-2 grid gap-4 sm:grid-cols-2">
          {daemon.settings.map((section) => (
            <section key={section.name} aria-label={section.name}>
              <h3 className="text-xs font-semibold tracking-wide text-slate-500 uppercase dark:text-slate-400">{section.name}</h3>
              <dl className="mt-1 space-y-0.5">
                {section.settings.map((setting) => (
                  <div key={setting.key} className="flex gap-2">
                    <dt className={`${term} w-36 shrink-0 font-mono text-[13px]`}>{setting.key}</dt>
                    <dd className={`${value} ${code}`}>{setting.value}</dd>
                  </div>
                ))}
              </dl>
            </section>
          ))}
        </div>
      </div>
    </>
  );
}

function Rows({ children }: { children: ReactNode }) {
  return <dl className="mt-2 space-y-2">{children}</dl>;
}

function Row({ name, children }: { name: string; children: ReactNode }) {
  return (
    <div className="grid gap-1 sm:grid-cols-[10rem_1fr]">
      <dt className={term}>{name}</dt>
      <dd className={value}>{children}</dd>
    </div>
  );
}

function formatDate(iso: string): string {
  const parsed = Date.parse(iso);
  return Number.isNaN(parsed) ? "unknown" : dateTime.format(parsed);
}
