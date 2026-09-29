#!/usr/bin/env perl
# Mechanical rebrand of the vendored Grok Build tree to vktr.
#
# Global rules (code, strings, comments, docs):
#   GROK_<X>        -> VKTR_<X>       env vars and constants
#   .grok (path)    -> .vktr          home/project dot-dirs; not .grok.com, .grok-plugin, .grok_home
#   Grok Build      -> vktr
# String-literal and comment-only rules (never touch identifiers):
#   `grok`          -> `vktr`         backticked CLI name
#   grok <subcmd>   -> vktr <subcmd>  CLI examples such as `grok login`, `grok -p`
#   Grok            -> vktr           product name; not model names like Grok 4 / Grok-4
#
# Usage: perl scripts/rebrand.pl <files...>   (edits in place)
use strict;
use warnings;

my $sub = qr/(?:login|logout|update|models|model|inspect|agent|ssh|mcp|plugin|plugins|skills|skill|config|doctor|completions|setup|resume|sessions|session|test-seams|workspace|memory|sandbox|auth|hooks|init|run|serve|version|help|-p\b|--[\w-]+|-[a-zA-Z]\b)/;

sub rebrand_text {
    my ($s) = @_;
    $s =~ s/\.grok(?![\w.-])/.vktr/g;
    $s =~ s/`grok`/`vktr`/g;
    # The bare CLI / product name anywhere in text: not part of an identifier, path, host, package or model id.
    $s =~ s/(?<![-\w\/.@])grok(?![-\w.\/])/vktr/g;
    $s =~ s/(?<![-\w])Grok(?![-\w]|\s?\d)/vktr/g;
    return $s;
}

for my $file (@ARGV) {
    open my $in, '<', $file or die "$file: $!";
    my @lines = <$in>;
    close $in;
    my $changed = 0;
    my $is_rust = $file =~ /\.rs$/;
    my $in_string = 0;    # inside a multi-line "..." literal carried over from a previous line
    for my $line (@lines) {
        my $orig = $line;
        $line =~ s/GROK_/VKTR_/g;
        $line =~ s/Grok Build/vktr/g;
        if ($is_rust) {
            if ($in_string) {
                # continuation of a multi-line string: text up to the closing quote is string content
                if ($line =~ /^((?:[^"\\]|\\.)*")(.*)$/s) {
                    my ($head, $tail) = ($1, $2);
                    $head = rebrand_text($head);
                    $tail =~ s/("(?:[^"\\]|\\.)*")/rebrand_text($1)/ge;
                    $line = $head . $tail;
                    $in_string = ($tail =~ /^(?:[^"\\]|\\.)*"(?:[^"\\]|\\.)*$/) ? 1 : 0;
                } else {
                    $line = rebrand_text($line);
                }
            } elsif ($line =~ /^\s*\/\//) {
                $line = rebrand_text($line);
            } else {
                # only inside string literals
                $line =~ s/("(?:[^"\\]|\\.)*")/rebrand_text($1)/ge;
                # an unmatched opening quote starts a multi-line string
                my $rest = $line;
                $rest =~ s/"(?:[^"\\]|\\.)*"//g;
                $rest =~ s/'"'//g;
                if ($rest =~ /"((?:[^"\\]|\\.)*)$/) {
                    my $open = $1;
                    my $branded = rebrand_text($open);
                    $line =~ s/\Q$open\E$/$branded/ if $branded ne $open;
                    $in_string = 1;
                }
            }
        } else {
            $line = rebrand_text($line);
        }
        $changed ||= ($line ne $orig);
    }
    if ($changed) {
        open my $out, '>', $file or die "$file: $!";
        print $out @lines;
        close $out;
    }
}
