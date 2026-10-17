# Coding agents

TeXoria works with the coding agents you already have on your computer: it can ask them to
write a section, rewrite a passage or fix what stops your document from typesetting. The agent
uses your own account, so there is nothing to set up in TeXoria and no API key to give it.

## Supported agents

Any agent that speaks the [Agent Client Protocol](https://agentclientprotocol.com) (ACP).
TeXoria finds these by itself, on your `PATH` and in the usual install folders:

| Agent | Installed as | How TeXoria starts it |
|---|---|---|
| Claude Code | `claude` | through the `@agentclientprotocol/claude-agent-acp` adapter (`npx`), or `claude-agent-acp` if installed |
| Codex | `codex` | through the `@agentclientprotocol/codex-acp` adapter (`npx`), or `codex-acp` if installed |
| Gemini CLI | `gemini` | `gemini --acp` |
| GitHub Copilot | `copilot` | `copilot --acp` |
| Cursor | `cursor-agent` | `cursor-agent acp` |
| OpenCode | `opencode` | `opencode acp` |
| goose | `goose` | `goose acp` |
| Qwen Code | `qwen` | `qwen --acp` |
| Kimi CLI | `kimi` | `kimi acp` |
| Mistral Vibe | `vibe-acp` | `vibe-acp` |
| Auggie | `auggie` | `auggie --acp` |

Claude Code and Codex need [Node.js](https://nodejs.org) (`npx`) for their adapter, which is
downloaded the first time: the first start can take a minute. Sign in to the agent from a
terminal first (`claude`, then `/login`; `codex login`; …): TeXoria shows how when the agent
asks for it, and offers the sign-in methods the agent supports itself.

Another agent can be added in **Settings → Agents → Other agents**: its command and the
arguments that start it in ACP mode. When several agents are installed, choose one there or
from the menu at the top of the Agent panel.

## The Agent panel

Open it with the ✦ button in the title bar, **Agent → Agent** in the menu bar, or ⇧⌘K
(Ctrl+Shift+K). It shares the sidebar with the outline, the files and source control.

The first time, TeXoria asks whether agents may work in the project: an agent reads and
changes its files, can run commands, sends what it reads to its provider, and follows its own
settings found in the project (which can run programs). Allow only projects you trust; you can
take it back in **Settings → Trusted Projects**.

Then write to the agent and press ↵ (⇧↵ for a new line). Each message comes with the open file,
and with the selected text if there is a selection. The panel shows the agent's answer as it
writes it, the files it reads and changes, the commands it runs and its plan. When it needs your
permission (to run a command, say), it asks in the chat with the choices it offers.

Agents work in a mode of their own (Claude Code: *Manual*, *Accept edits*, *Plan*, …): the
menu next to the agent's name changes it, when the agent has modes. **New chat** starts over.

## Quick edits

Select a passage, right-click and pick **Agent**:

- **Improve Writing**: clearer, with a better flow, saying the same thing.
- **Make Shorter**
- **Make More Formal**: the tone of a scientific publication.
- **Fix Grammar and Spelling**: nothing else changes.
- **Edit with Instructions…** (⌘K, Ctrl+K): say what to do, like "translate into English" or
  "turn this list into a table".

The agent edits the file in place and keeps the LaTeX commands, labels, citations and math.

## Fixing typesetting problems

When typesetting fails, **Fix with Agent** in the Problems panel (⌘⇧M), the button in an empty
chat or **Agent → Fix Typesetting Problems** sends the errors (or, without errors, the
warnings) to the agent. It typesets again with TeXoria's engine to check its fix: you see the
PDF update as it works.

## Reviewing what the agent changed

While the agent works, editing is paused: the editor is read-only and a bar above it lets you
stop the agent. When it is done, the panel shows how many files changed:

- **Review** opens the changes in the diff tab, file by file, each with its own **Undo**.
- **Undo** puts every file back as it was before the agent's turn, new files included.
- **Keep** accepts them.

The next message keeps the changes of the previous turn. Undo only covers the project folder
(not `.git` or `.texoria`), and files larger than 8 MB cannot be put back. If the agent changes
Git's configuration or hooks, which can run programs, the chat says so: check them before using
Git in the project.

When an agent reads or writes files through TeXoria, it can only reach the project folder, not
its `.git`, and it cannot write `latexmkrc` (a program that runs when typesetting): change that
file yourself. Agents can still run commands of their own, with your permission.

## TeXoria's tools

TeXoria gives each agent three tools, over MCP, under the name `texoria`:

| Tool | |
|---|---|
| `typeset` | Save and typeset with the engine chosen in TeXoria, and get the errors and warnings. |
| `problems` | The errors and warnings of the last run. |
| `outline` | The document's parts, sections, figures and tables, with their file and line. |

The agent starts them with `texoria --mcp-bridge`, which only talks to the TeXoria window that
started the agent.

## Troubleshooting

- **No agent found**: TeXoria looks on your shell's `PATH` and in the usual folders
  (`~/.local/bin`, Homebrew, npm, nvm, Bun, Volta, …). Check that the agent runs in a terminal,
  then choose **Look for Agents Again** in the panel's menu, or add it in the settings.
- **The agent stopped**: the panel shows the end of what the agent printed. **Start Again**
  starts it again. With Claude Code and Codex, check that `npx` works.
- **It asks to sign in**: sign in from a terminal as shown, then send the message again.
