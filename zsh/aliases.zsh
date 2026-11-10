# ~/.config/zsh/aliases.zsh - sourced from ~/.zshrc (after oh-my-zsh)
# Aliases that use optional tools (eza, fd, zoxide, fzf, btop, trash) only
# switch on when the tool is installed; otherwise the plain command is used.

# --- navigation ----------------------------------------------------------------
alias ..='cd ..'
alias ...='cd ../..'
alias ....='cd ../../..'
alias -- -='cd -'
alias dl='cd ~/Downloads'
alias rice='cd ~/rice'

# zoxide: `z foo` jumps to the most used dir matching "foo", `zi` picks interactively
(( $+commands[zoxide] )) && eval "$(zoxide init zsh)"

# --- listing -----------------------------------------------------------------------
if (( $+commands[eza] )); then
  alias ls='eza --icons --group-directories-first'
  alias l='eza -l --icons --group-directories-first --no-user --time-style=relative'
  alias ll='eza -la --icons --group-directories-first --git --time-style=relative'
  alias la='eza -a --icons --group-directories-first'
  alias lt='eza --tree --level=2 --icons --group-directories-first'
else
  alias ls='ls --color=auto --group-directories-first'
  alias l='ls -lh'
  alias ll='ls -lah'
  alias la='ls -A'
  alias lt='ls -R'
fi

# --- files ---------------------------------------------------------------------------
export BAT_THEME="Catppuccin Mocha"
alias cat='bat --paging=never --style=plain'
alias catn='bat --paging=never'                 # with line numbers + git changes
alias cp='cp -iv'
alias mv='mv -iv'
alias mkdir='mkdir -pv'
(( $+commands[trash-put] )) && alias rm='trash-put' || alias rm='rm -I'
alias size='du -sh'
alias usage='du -h -d1 | sort -h'

# --- search --------------------------------------------------------------------------
alias grep='grep --color=auto'
# fzf: Ctrl+R history search, Ctrl+T file picker, Alt+C cd into dir
(( $+commands[fzf] )) && source <(fzf --zsh)
export FZF_DEFAULT_OPTS="\
  --height=40% --layout=reverse --border=rounded --info=inline-right \
  --color=bg+:#313244,bg:#11111b,spinner:#f5e0dc,hl:#f38ba8 \
  --color=fg:#cdd6f4,header:#f38ba8,info:#cba6f7,pointer:#cba6f7 \
  --color=marker:#b4befe,fg+:#cdd6f4,prompt:#cba6f7,hl+:#f38ba8 \
  --color=selected-bg:#45475a,border:#313244,label:#cdd6f4"
(( $+commands[fd] )) && export FZF_DEFAULT_COMMAND='fd --type f --hidden --exclude .git' FZF_CTRL_T_COMMAND='fd --type f --hidden --exclude .git' FZF_ALT_C_COMMAND='fd --type d --hidden --exclude .git'

# --- packages (dnf + flatpak) ----------------------------------------------------------
alias up='sudo dnf upgrade --refresh -y && flatpak update -y'
alias ins='sudo dnf install'
alias un='sudo dnf remove'
alias search='dnf search'
alias pkginfo='dnf info'
alias cleanup='sudo dnf autoremove -y && flatpak uninstall --unused -y'

# --- system --------------------------------------------------------------------------
alias ff='fastfetch'
alias df='df -h'
alias free='free -h'
alias ports='ss -tulpn'
alias myip='curl -s https://ifconfig.me; echo'
(( $+commands[btop] )) && alias top='btop'
alias logs='journalctl -p 3 -xb'                # errors from this boot

# --- shell ---------------------------------------------------------------------------
alias c='clear'
alias h='history'
alias reload='exec zsh'
alias zshrc='code ~/.zshrc'
alias aliases='code ~/.config/zsh/aliases.zsh'
alias path='print -l $path'

# --- editor --------------------------------------------------------------------------
alias c.='code .'

# --- theme (MacTahoe Catppuccin) -----------------------------------------------------------
# rebuild + install the GTK/shell theme with our options
alias retheme='~/rice/gnome/install-theme.sh'
# reload the GNOME Shell theme without logging out
alias reshell='gsettings --schemadir ~/.local/share/gnome-shell/extensions/user-theme@gnome-shell-extensions.gcampax.github.com/schemas set org.gnome.shell.extensions.user-theme name "" && sleep 1 && gsettings --schemadir ~/.local/share/gnome-shell/extensions/user-theme@gnome-shell-extensions.gcampax.github.com/schemas set org.gnome.shell.extensions.user-theme name MacTahoe-Dark-solid'
