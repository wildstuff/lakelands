#!/usr/bin/env perl
# Makes brand/tokens.css from brand/tokens.json: one custom property per
# token, the first theme on `:root`, every further theme as the values
# that differ from it, and one class per type style. The stylesheet is
# generated, never edited; the `brand-tokens` gate holds the two files
# to each other.
#
# Usage: perl scripts/brand/tokens-css.pl brand/tokens.json > brand/tokens.css
use strict;
use warnings;
use JSON::PP;

my $path = shift @ARGV or die "usage: tokens-css.pl <tokens.json>\n";
open my $fh, '<:raw', $path or die "cannot read $path: $!\n";
my $tokens = JSON::PP->new->utf8->decode(do { local $/; <$fh> });
close $fh;

# An alias `{name}` reads as the custom property of that name.
sub css_value {
    my ($value) = @_;
    return $value =~ /^\{([A-Za-z0-9][A-Za-z0-9_.-]*)\}$/ ? "var(--$1)" : $value;
}

my @themes = map { $_->{id} } @{ $tokens->{color}{themes} };
my $first = $themes[0];
my @colors = @{ $tokens->{color}{tokens} };

sub theme_value {
    my ($token, $theme) = @_;
    my $v = $token->{value};
    return $v unless ref $v;
    return defined $v->{$theme} ? $v->{$theme} : $v->{$first};
}

my $out = "/* generated from brand/tokens.json by scripts/brand/tokens-css.pl; do not edit */\n";

$out .= ":root, [data-theme=\"$first\"] {\n";
$out .= "  --$_->{name}: " . css_value(theme_value($_, $first)) . ";\n" for @colors;
$out .= "}\n";

for my $theme (@themes[1 .. $#themes]) {
    my @differ = grep {
        ref $_->{value} && defined $_->{value}{$theme} && $_->{value}{$theme} ne theme_value($_, $first)
    } @colors;
    next unless @differ;
    $out .= "[data-theme=\"$theme\"] {\n";
    $out .= "  --$_->{name}: " . css_value($_->{value}{$theme}) . ";\n" for @differ;
    $out .= "}\n";
}

$out .= ":root {\n";
for my $family (qw(spacing radius)) {
    next unless $tokens->{$family};
    $out .= "  --$_->{name}: $_->{value};\n" for @{ $tokens->{$family}{tokens} };
}
my $families = $tokens->{type}{families} || {};
$out .= "  --font-$_: $families->{$_};\n" for sort keys %$families;
$out .= "}\n";

for my $group (@{ $tokens->{type}{groups} || [] }) {
    for my $style (@{ $group->{styles} }) {
        my $family = $style->{family} || $group->{family};
        $out .= ".$style->{name} {\n";
        $out .= "  font-family: var(--font-$family);\n";
        $out .= "  font-size: $style->{fontSize};\n";
        $out .= "  line-height: $style->{lineHeight};\n" if defined $style->{lineHeight};
        $out .= "  font-weight: $style->{fontWeight};\n" if defined $style->{fontWeight};
        $out .= "  letter-spacing: $style->{letterSpacing};\n" if defined $style->{letterSpacing};
        $out .= "}\n";
    }
}

binmode STDOUT, ':raw';
print $out;
