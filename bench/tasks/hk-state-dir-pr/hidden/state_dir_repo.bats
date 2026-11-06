#!/usr/bin/env bats

# The review of jdx/hk#1705, as tests the agent never sees: state follows the
# repository hk operates on (GIT_DIR / GIT_WORK_TREE, as `Git::new()` resolves
# it), and a repository shared between accounts (core.sharedRepository) does
# not get a state directory only its first user can write to.

setup() {
    load 'test_helper/common_setup'
    _common_setup
    unset HK_STATE_DIR HK_OUTPUT_FILE HK_LOG_FILE
}

teardown() {
    _common_teardown
}

write_failing_config() {
    cat <<EOF > hk.pkl
amends "$PKL_PATH/Config.pkl"
hooks {
  ["check"] {
    steps {
      ["fail"] {
        check = "echo STATE_MARKER 1>&2 && exit 1"
        glob = List("*.txt")
      }
    }
  }
}
EOF
    echo "content" > file.txt
}

@test "state follows GIT_DIR and GIT_WORK_TREE, not the repository around the current directory" {
    # The current directory is in one repository (proj/.git); GIT_DIR names
    # another, YADM-style, with this directory as its work tree.
    write_failing_config
    other="$BATS_TEST_TMPDIR/dotfiles.git"
    git init -q --bare "$other"
    export GIT_DIR="$other" GIT_WORK_TREE="$PWD"
    git add hk.pkl file.txt
    git commit -q -m init

    run hk check --all
    assert_failure
    # Anywhere under its hk/ counts: the review lets each account keep its
    # own directory there.
    grep -rqs STATE_MARKER "$other/hk"
    if grep -rqs STATE_MARKER "$PWD/.git/hk"; then
        echo "hk saved the output in the repository around the current directory" >&2
        return 1
    fi
}

@test "a repository shared between accounts gets no hk state directory private to one account" {
    write_failing_config
    git config core.sharedRepository group
    git add hk.pkl file.txt
    git commit -q -m init

    run hk check --all
    assert_failure
    # hk still saved the output somewhere and said where.
    log="$(printf '%s\n' "$output" | sed -n 's/.*See \(.*\) for full command output.*/\1/p' | tail -n 1)"
    [ -n "$log" ]
    assert_file_contains "$log" "STATE_MARKER"
    # Another account in the group must be able to write hk's state too: any
    # hk directory in the shared git dir is group-writable and searchable.
    for dir in .git/hk .git/hk/patches; do
        [ -d "$dir" ] || continue
        mode="$(stat -c %a "$dir" 2>/dev/null || stat -f %Lp "$dir")"
        if (((8#$mode & 8#070) != 8#070)); then
            echo "$dir is mode $mode: other accounts in the group cannot write to it" >&2
            return 1
        fi
    done
}
