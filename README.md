# wm-dash

A full-screen dashboard for [workmux](https://github.com/raine/workmux) agents: the agent list on the left and a live preview of the selected agent's tmux pane on the right.

Agents that need you come first: 💬 waiting, ✅ done, ⏳ idle with a background task, 🤖 working.

Run it in a tmux popup:

```tmux
bind w popup -h 100% -w 100% -E "wm-dash"
```

## Keys

In the list:

| Key | Action |
| --- | --- |
| `j` / `k`, arrows, click | Move the selection |
| `PgUp` / `PgDn`, mouse wheel | Scroll the preview (scrolls the agent's own pane) |
| Mouse over the preview | Select text and click, as in the agent's pane |
| `i` | Insert mode: keys go to the agent |
| `enter` | Jump to the agent's pane and quit |
| `l` | Open lazygit in the agent's working directory |
| `v` | Open nvim in the agent's working directory |
| `g` | Open the branch's GitHub PR in the browser, or the page to create one |
| `r` | Remove the agent and its worktree (asks first) |
| `?` | Show all keys |
| `q`, `ctrl+c` | Quit |

In insert mode, every key and paste goes to the agent. `ctrl+q` takes you back to the list.

## Build

```sh
nix build            # result/bin/wm-dash
nix develop -c cargo test
```

Use it from another flake by adding this repository as an input and installing `inputs.wm-dash.packages.${system}.default`.
