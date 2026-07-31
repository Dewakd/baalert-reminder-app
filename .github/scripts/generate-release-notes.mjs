import { execFileSync } from "node:child_process";
import { appendFileSync, readFileSync } from "node:fs";

const CONVENTIONAL_COMMIT = /^(\w+)(?:\(([^)]+)\))?(!)?:\s+(.+)$/;
const VERSION_TAG = /^v\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/;

function git(args) {
  return execFileSync("git", args, { encoding: "utf8" }).trim();
}

function capitalize(value) {
  return value ? value[0].toUpperCase() + value.slice(1) : value;
}

function parseCommit(record) {
  const [sha, subject, body = ""] = record.split("\x1f");
  const match = subject.match(CONVENTIONAL_COMMIT);
  const breaking = Boolean(match?.[3]) || /BREAKING CHANGE:/i.test(body);

  return {
    sha,
    subject,
    type: match?.[1]?.toLowerCase() ?? "other",
    scope: match?.[2] ?? "",
    description: capitalize(match?.[4] ?? subject),
    breaking,
  };
}

function categoryFor(commit) {
  if (commit.breaking) return "Breaking changes";

  return (
    {
      feat: "Features",
      fix: "Fixes",
      perf: "Performance",
      refactor: "Improvements",
      revert: "Improvements",
      docs: "Documentation",
    }[commit.type] ?? "Maintenance"
  );
}

function commitBullet(commit, repository) {
  const scope = commit.scope ? `**${commit.scope}:** ` : "";
  const reference = repository
    ? ` ([${commit.sha.slice(0, 7)}](https://github.com/${repository}/commit/${commit.sha}))`
    : ` (${commit.sha.slice(0, 7)})`;
  return `- ${scope}${commit.description}${reference}`;
}

function generateNotes() {
  const packageVersion = JSON.parse(readFileSync("package.json", "utf8")).version;
  const currentTag =
    process.env.GITHUB_REF_TYPE === "tag" && VERSION_TAG.test(process.env.GITHUB_REF_NAME ?? "")
      ? process.env.GITHUB_REF_NAME
      : `v${packageVersion}`;
  const tags = git(["tag", "--merged", "HEAD", "--sort=-version:refname"])
    .split("\n")
    .filter((tag) => VERSION_TAG.test(tag));
  const previousTag = tags.find((tag) => tag !== currentTag);
  const range = previousTag ? `${previousTag}..HEAD` : "HEAD";
  const rawLog = git(["log", range, "--pretty=format:%H%x1f%s%x1f%b%x1e"]);
  const commits = rawLog
    .split("\x1e")
    .map((record) => record.trim())
    .filter(Boolean)
    .map(parseCommit);
  const repository = process.env.GITHUB_REPOSITORY ?? "";
  const categoryOrder = [
    "Breaking changes",
    "Features",
    "Fixes",
    "Performance",
    "Improvements",
    "Documentation",
    "Maintenance",
  ];
  const sections = categoryOrder.flatMap((category) => {
    const matching = commits.filter((commit) => categoryFor(commit) === category);
    if (matching.length === 0) return [];
    return [
      `### ${category}`,
      ...matching.map((commit) => commitBullet(commit, repository)),
      "",
    ];
  });

  return [
    "## Release summary",
    previousTag ? `Changes since \`${previousTag}\`.` : "Changes included in this release.",
    "",
    ...(sections.length > 0 ? sections : ["- Packaging and release updates.", ""]),
  ]
    .join("\n")
    .trim();
}

const notes = generateNotes();
const outputPath = process.env.GITHUB_OUTPUT;

if (outputPath) {
  const delimiter = `BAALERT_RELEASE_NOTES_${Date.now()}`;
  appendFileSync(outputPath, `body<<${delimiter}\n${notes}\n${delimiter}\n`);
}

console.log(notes);
