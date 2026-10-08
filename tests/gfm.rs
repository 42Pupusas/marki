//! GFM conformance: the extension examples of the GFM spec and cmark-gfm's
//! `extensions.txt`, plus a differential check against comrak with the same
//! extensions enabled. cmark-gfm is the reference; comrak is used only for
//! documents outside the fixture, and differs from cmark-gfm on two autolink
//! cases in `ext autolinks`. Built only with the `gfm` feature.

#![cfg(feature = "gfm")]

use marki_parse::MarkdownFile;

/// One GFM example: markdown input and the HTML cmark-gfm produces for it.
struct Example {
    name: &'static str,
    markdown: &'static str,
    html: &'static str,
}

/// The extension examples, verbatim from the GFM spec and cmark-gfm
/// `test/extensions.txt` (tabs written as `\t`).
struct Corpus;

impl Corpus {
    const EXAMPLES: &'static [Example] = &[
        Example {
            name: "spec table 198",
            markdown: "| foo | bar |\n| --- | --- |\n| baz | bim |\n",
            html: "<table>\n<thead>\n<tr>\n<th>foo</th>\n<th>bar</th>\n</tr>\n</thead>\n<tbody>\n<tr>\n<td>baz</td>\n<td>bim</td>\n</tr>\n</tbody>\n</table>\n",
        },
        Example {
            name: "spec table 199",
            markdown: "| abc | defghi |\n:-: | -----------:\nbar | baz\n",
            html: "<table>\n<thead>\n<tr>\n<th align=\"center\">abc</th>\n<th align=\"right\">defghi</th>\n</tr>\n</thead>\n<tbody>\n<tr>\n<td align=\"center\">bar</td>\n<td align=\"right\">baz</td>\n</tr>\n</tbody>\n</table>\n",
        },
        Example {
            name: "spec table 200",
            markdown: "| f\\|oo  |\n| ------ |\n| b `\\|` az |\n| b **\\|** im |\n",
            html: "<table>\n<thead>\n<tr>\n<th>f|oo</th>\n</tr>\n</thead>\n<tbody>\n<tr>\n<td>b <code>|</code> az</td>\n</tr>\n<tr>\n<td>b <strong>|</strong> im</td>\n</tr>\n</tbody>\n</table>\n",
        },
        Example {
            name: "spec table 201",
            markdown: "| abc | def |\n| --- | --- |\n| bar | baz |\n> bar\n",
            html: "<table>\n<thead>\n<tr>\n<th>abc</th>\n<th>def</th>\n</tr>\n</thead>\n<tbody>\n<tr>\n<td>bar</td>\n<td>baz</td>\n</tr>\n</tbody>\n</table>\n<blockquote>\n<p>bar</p>\n</blockquote>\n",
        },
        Example {
            name: "spec table 202",
            markdown: "| abc | def |\n| --- | --- |\n| bar | baz |\nbar\n\nbar\n",
            html: "<table>\n<thead>\n<tr>\n<th>abc</th>\n<th>def</th>\n</tr>\n</thead>\n<tbody>\n<tr>\n<td>bar</td>\n<td>baz</td>\n</tr>\n<tr>\n<td>bar</td>\n<td></td>\n</tr>\n</tbody>\n</table>\n<p>bar</p>\n",
        },
        Example {
            name: "spec table 203",
            markdown: "| abc | def |\n| --- |\n| bar |\n",
            html: "<p>| abc | def |\n| --- |\n| bar |</p>\n",
        },
        Example {
            name: "spec table 204",
            markdown: "| abc | def |\n| --- | --- |\n| bar |\n| bar | baz | boo |\n",
            html: "<table>\n<thead>\n<tr>\n<th>abc</th>\n<th>def</th>\n</tr>\n</thead>\n<tbody>\n<tr>\n<td>bar</td>\n<td></td>\n</tr>\n<tr>\n<td>bar</td>\n<td>baz</td>\n</tr>\n</tbody>\n</table>\n",
        },
        Example {
            name: "spec table 205",
            markdown: "| abc | def |\n| --- | --- |\n",
            html: "<table>\n<thead>\n<tr>\n<th>abc</th>\n<th>def</th>\n</tr>\n</thead>\n</table>\n",
        },
        Example {
            name: "spec tasklist 279",
            markdown: "- [ ] foo\n- [x] bar\n",
            html: "<ul>\n<li><input type=\"checkbox\" disabled=\"\" /> foo</li>\n<li><input type=\"checkbox\" checked=\"\" disabled=\"\" /> bar</li>\n</ul>\n",
        },
        Example {
            name: "spec tasklist 280",
            markdown: "- [x] foo\n  - [ ] bar\n  - [x] baz\n- [ ] bim\n",
            html: "<ul>\n<li><input type=\"checkbox\" checked=\"\" disabled=\"\" /> foo\n<ul>\n<li><input type=\"checkbox\" disabled=\"\" /> bar</li>\n<li><input type=\"checkbox\" checked=\"\" disabled=\"\" /> baz</li>\n</ul>\n</li>\n<li><input type=\"checkbox\" disabled=\"\" /> bim</li>\n</ul>\n",
        },
        Example {
            name: "spec strikethrough 491",
            markdown: "~~Hi~~ Hello, world!\n",
            html: "<p><del>Hi</del> Hello, world!</p>\n",
        },
        Example {
            name: "spec strikethrough 492",
            markdown: "This ~~has a\n\nnew paragraph~~.\n",
            html: "<p>This ~~has a</p>\n<p>new paragraph~~.</p>\n",
        },
        Example {
            name: "spec autolink 621",
            markdown: "www.commonmark.org\n",
            html: "<p><a href=\"http://www.commonmark.org\">www.commonmark.org</a></p>\n",
        },
        Example {
            name: "spec autolink 622",
            markdown: "Visit www.commonmark.org/help for more information.\n",
            html: "<p>Visit <a href=\"http://www.commonmark.org/help\">www.commonmark.org/help</a> for more information.</p>\n",
        },
        Example {
            name: "spec autolink 623",
            markdown: "Visit www.commonmark.org.\n\nVisit www.commonmark.org/a.b.\n",
            html: "<p>Visit <a href=\"http://www.commonmark.org\">www.commonmark.org</a>.</p>\n<p>Visit <a href=\"http://www.commonmark.org/a.b\">www.commonmark.org/a.b</a>.</p>\n",
        },
        Example {
            name: "spec autolink 624",
            markdown: "www.google.com/search?q=Markup+(business)\n\nwww.google.com/search?q=Markup+(business)))\n\n(www.google.com/search?q=Markup+(business))\n\n(www.google.com/search?q=Markup+(business)\n",
            html: "<p><a href=\"http://www.google.com/search?q=Markup+(business)\">www.google.com/search?q=Markup+(business)</a></p>\n<p><a href=\"http://www.google.com/search?q=Markup+(business)\">www.google.com/search?q=Markup+(business)</a>))</p>\n<p>(<a href=\"http://www.google.com/search?q=Markup+(business)\">www.google.com/search?q=Markup+(business)</a>)</p>\n<p>(<a href=\"http://www.google.com/search?q=Markup+(business)\">www.google.com/search?q=Markup+(business)</a></p>\n",
        },
        Example {
            name: "spec autolink 625",
            markdown: "www.google.com/search?q=(business))+ok\n",
            html: "<p><a href=\"http://www.google.com/search?q=(business))+ok\">www.google.com/search?q=(business))+ok</a></p>\n",
        },
        Example {
            name: "spec autolink 626",
            markdown: "www.google.com/search?q=commonmark&hl=en\n\nwww.google.com/search?q=commonmark&hl;\n",
            html: "<p><a href=\"http://www.google.com/search?q=commonmark&amp;hl=en\">www.google.com/search?q=commonmark&amp;hl=en</a></p>\n<p><a href=\"http://www.google.com/search?q=commonmark\">www.google.com/search?q=commonmark</a>&amp;hl;</p>\n",
        },
        Example {
            name: "spec autolink 627",
            markdown: "www.commonmark.org/he<lp\n",
            html: "<p><a href=\"http://www.commonmark.org/he\">www.commonmark.org/he</a>&lt;lp</p>\n",
        },
        Example {
            name: "spec autolink 628",
            markdown: "http://commonmark.org\n\n(Visit https://encrypted.google.com/search?q=Markup+(business))\n\nAnonymous FTP is available at ftp://foo.bar.baz.\n",
            html: "<p><a href=\"http://commonmark.org\">http://commonmark.org</a></p>\n<p>(Visit <a href=\"https://encrypted.google.com/search?q=Markup+(business)\">https://encrypted.google.com/search?q=Markup+(business)</a>)</p>\n<p>Anonymous FTP is available at <a href=\"ftp://foo.bar.baz\">ftp://foo.bar.baz</a>.</p>\n",
        },
        Example {
            name: "spec autolink 629",
            markdown: "foo@bar.baz\n",
            html: "<p><a href=\"mailto:foo@bar.baz\">foo@bar.baz</a></p>\n",
        },
        Example {
            name: "spec autolink 630",
            markdown: "hello@mail+xyz.example isn't valid, but hello+xyz@mail.example is.\n",
            html: "<p>hello@mail+xyz.example isn't valid, but <a href=\"mailto:hello+xyz@mail.example\">hello+xyz@mail.example</a> is.</p>\n",
        },
        Example {
            name: "spec autolink 631",
            markdown: "a.b-c_d@a.b\n\na.b-c_d@a.b.\n\na.b-c_d@a.b-\n\na.b-c_d@a.b_\n",
            html: "<p><a href=\"mailto:a.b-c_d@a.b\">a.b-c_d@a.b</a></p>\n<p><a href=\"mailto:a.b-c_d@a.b\">a.b-c_d@a.b</a>.</p>\n<p>a.b-c_d@a.b-</p>\n<p>a.b-c_d@a.b_</p>\n",
        },
        Example {
            name: "spec tagfilter 652",
            markdown: "<strong> <title> <style> <em>\n\n<blockquote>\n  <xmp> is disallowed.  <XMP> is also disallowed.\n</blockquote>\n",
            html: "<p><strong> &lt;title> &lt;style> <em></p>\n<blockquote>\n  &lt;xmp> is disallowed.  &lt;XMP> is also disallowed.\n</blockquote>\n",
        },
        Example {
            name: "ext tables mixed",
            markdown: "Hello!\n\n| _abc_ | セン |\n| ----- | ---- |\n| 1. Block elements inside cells don't work. | |\n| But _**inline elements do**_. | x |\n\nHi!\n",
            html: "<p>Hello!</p>\n<table>\n<thead>\n<tr>\n<th><em>abc</em></th>\n<th>セン</th>\n</tr>\n</thead>\n<tbody>\n<tr>\n<td>1. Block elements inside cells don't work.</td>\n<td></td>\n</tr>\n<tr>\n<td>But <em><strong>inline elements do</strong></em>.</td>\n<td>x</td>\n</tr>\n</tbody>\n</table>\n<p>Hi!</p>\n",
        },
        Example {
            name: "ext tables edge cases",
            markdown: "| Not enough table | to be considered table |\n\n| Not enough table | to be considered table |\n| Not enough table | to be considered table |\n\n| Just enough table | to be considered table |\n| ----------------- | ---------------------- |\n\n| ---- | --- |\n\n|x|\n|-|\n\n| xyz |\n| --- |\n",
            html: "<p>| Not enough table | to be considered table |</p>\n<p>| Not enough table | to be considered table |\n| Not enough table | to be considered table |</p>\n<table>\n<thead>\n<tr>\n<th>Just enough table</th>\n<th>to be considered table</th>\n</tr>\n</thead>\n</table>\n<p>| ---- | --- |</p>\n<table>\n<thead>\n<tr>\n<th>x</th>\n</tr>\n</thead>\n</table>\n<table>\n<thead>\n<tr>\n<th>xyz</th>\n</tr>\n</thead>\n</table>\n",
        },
        Example {
            name: "ext tables trailing space",
            markdown: "Hello!\n\n| _abc_ | セン |\n| ----- | ---- |\n| this row has a space at the end | | \n| But _**inline elements do**_. | x |\n\nHi!\n",
            html: "<p>Hello!</p>\n<table>\n<thead>\n<tr>\n<th><em>abc</em></th>\n<th>セン</th>\n</tr>\n</thead>\n<tbody>\n<tr>\n<td>this row has a space at the end</td>\n<td></td>\n</tr>\n<tr>\n<td>But <em><strong>inline elements do</strong></em>.</td>\n<td>x</td>\n</tr>\n</tbody>\n</table>\n<p>Hi!</p>\n",
        },
        Example {
            name: "ext tables row counts",
            markdown: "| a | b | c |\n| --- | --- | ---\n| x\n| a | b\n| 1 | 2 | 3 | 4 | 5 |\n",
            html: "<table>\n<thead>\n<tr>\n<th>a</th>\n<th>b</th>\n<th>c</th>\n</tr>\n</thead>\n<tbody>\n<tr>\n<td>x</td>\n<td></td>\n<td></td>\n</tr>\n<tr>\n<td>a</td>\n<td>b</td>\n<td></td>\n</tr>\n<tr>\n<td>1</td>\n<td>2</td>\n<td>3</td>\n</tr>\n</tbody>\n</table>\n",
        },
        Example {
            name: "ext tables embedded pipes",
            markdown: "| a | b |\n| --- | --- |\n| Escaped pipes are \\|okay\\|. | Like \\| this. |\n| Within `\\|code\\| is okay` too. |\n| _**`c\\|`**_ \\| complex\n| don't **\\_reparse\\_**\n",
            html: "<table>\n<thead>\n<tr>\n<th>a</th>\n<th>b</th>\n</tr>\n</thead>\n<tbody>\n<tr>\n<td>Escaped pipes are |okay|.</td>\n<td>Like | this.</td>\n</tr>\n<tr>\n<td>Within <code>|code| is okay</code> too.</td>\n<td></td>\n</tr>\n<tr>\n<td><em><strong><code>c|</code></strong></em> | complex</td>\n<td></td>\n</tr>\n<tr>\n<td>don't <strong>_reparse_</strong></td>\n<td></td>\n</tr>\n</tbody>\n</table>\n",
        },
        Example {
            name: "ext tables odd markers",
            markdown: "| a |\n--- |\n",
            html: "<table>\n<thead>\n<tr>\n<th>a</th>\n</tr>\n</thead>\n</table>\n",
        },
        Example {
            name: "ext tables escaping",
            markdown: "| a | b |\n| --- | --- |\n| \\\\ | `\\\\` |\n| \\\\\\\\ | `\\\\\\\\` |\n| \\_ | `\\_` |\n| \\| | `\\|` |\n| \\a | `\\a` |\n\n\\\\ `\\\\`\n\n\\\\\\\\ `\\\\\\\\`\n\n\\_ `\\_`\n\n\\| `\\|`\n\n\\a `\\a`\n",
            html: "<table>\n<thead>\n<tr>\n<th>a</th>\n<th>b</th>\n</tr>\n</thead>\n<tbody>\n<tr>\n<td>\\</td>\n<td><code>\\\\</code></td>\n</tr>\n<tr>\n<td>\\\\</td>\n<td><code>\\\\\\\\</code></td>\n</tr>\n<tr>\n<td>_</td>\n<td><code>\\_</code></td>\n</tr>\n<tr>\n<td>|</td>\n<td><code>|</code></td>\n</tr>\n<tr>\n<td>\\a</td>\n<td><code>\\a</code></td>\n</tr>\n</tbody>\n</table>\n<p>\\ <code>\\\\</code></p>\n<p>\\\\ <code>\\\\\\\\</code></p>\n<p>_ <code>\\_</code></p>\n<p>| <code>\\|</code></p>\n<p>\\a <code>\\a</code></p>\n",
        },
        Example {
            name: "ext tables embedded html",
            markdown: "| a |\n| --- |\n| <strong>hello</strong> |\n| ok <br> sure |\n",
            html: "<table>\n<thead>\n<tr>\n<th>a</th>\n</tr>\n</thead>\n<tbody>\n<tr>\n<td><strong>hello</strong></td>\n</tr>\n<tr>\n<td>ok <br> sure</td>\n</tr>\n</tbody>\n</table>\n",
        },
        Example {
            name: "ext tables reference links",
            markdown: "Here's a link to [Freedom Planet 2][].\n\n| Here's a link to [Freedom Planet 2][] in a table header. |\n| --- |\n| Here's a link to [Freedom Planet 2][] in a table row. |\n\n[Freedom Planet 2]: http://www.freedomplanet2.com/\n",
            html: "<p>Here's a link to <a href=\"http://www.freedomplanet2.com/\">Freedom Planet 2</a>.</p>\n<table>\n<thead>\n<tr>\n<th>Here's a link to <a href=\"http://www.freedomplanet2.com/\">Freedom Planet 2</a> in a table header.</th>\n</tr>\n</thead>\n<tbody>\n<tr>\n<td>Here's a link to <a href=\"http://www.freedomplanet2.com/\">Freedom Planet 2</a> in a table row.</td>\n</tr>\n</tbody>\n</table>\n",
        },
        Example {
            name: "ext tables sequential cells",
            markdown: "| a | b | c |\n| --- | --- | --- |\n| d || e |\n",
            html: "<table>\n<thead>\n<tr>\n<th>a</th>\n<th>b</th>\n<th>c</th>\n</tr>\n</thead>\n<tbody>\n<tr>\n<td>d</td>\n<td></td>\n<td>e</td>\n</tr>\n</tbody>\n</table>\n",
        },
        Example {
            name: "ext tables emphasis",
            markdown: "| a | b |\n| --- | --- |\n|***(a)***|\n",
            html: "<table>\n<thead>\n<tr>\n<th>a</th>\n<th>b</th>\n</tr>\n</thead>\n<tbody>\n<tr>\n<td><em><strong>(a)</strong></em></td>\n<td></td>\n</tr>\n</tbody>\n</table>\n",
        },
        Example {
            name: "ext strikethrough single",
            markdown: "A proper ~strikethrough~.\n",
            html: "<p>A proper <del>strikethrough</del>.</p>\n",
        },
        Example {
            name: "ext strikethrough edge cases",
            markdown: "These are ~not strikethroughs.\n\nNo, they are not~\n\nThis ~is ~ legit~ isn't ~ legit.\n\nThis is not ~~~~~one~~~~~ huge strikethrough.\n\n~one~ ~~two~~ ~~~three~~~\n\nNo ~mismatch~~\n",
            html: "<p>These are ~not strikethroughs.</p>\n<p>No, they are not~</p>\n<p>This <del>is ~ legit</del> isn't ~ legit.</p>\n<p>This is not ~~~~~one~~~~~ huge strikethrough.</p>\n<p><del>one</del> <del>two</del> ~~~three~~~</p>\n<p>No ~mismatch~~</p>\n",
        },
        Example {
            name: "ext autolinks",
            markdown: ": http://google.com https://google.com\n\n<http://google.com/å> http://google.com/å\n\nscyther@pokemon.com\n\nscy.the_rbe-edr+ill@pokemon.com\n\nscyther@pokemon.com.\n\nscyther@pokemon.com/\n\nscyther@pokemon.com/beedrill@pokemon.com\n\nmailto:scyther@pokemon.com\n\nThis is a mailto:scyther@pokemon.com\n\nmailto:scyther@pokemon.com.\n\nmmmmailto:scyther@pokemon.com\n\nmailto:scyther@pokemon.com/\n\nmailto:scyther@pokemon.com/message\n\nmailto:scyther@pokemon.com/mailto:beedrill@pokemon.com\n\nxmpp:scyther@pokemon.com\n\nxmpp:scyther@pokemon.com.\n\nxmpp:scyther@pokemon.com/message\n\nxmpp:scyther@pokemon.com/message.\n\nEmail me at:scyther@pokemon.com\n\nwww.github.com www.github.com/á\n\nwww.google.com/a_b\n\nUnderscores not allowed in host name www.xxx.yyy._zzz\n\nUnderscores not allowed in host name www.xxx._yyy.zzz\n\nUnderscores allowed in domain name www._xxx.yyy.zzz\n\n**Autolink and http://inlines**\n\n![http://inline.com/image](http://inline.com/image)\n\na.w@b.c\n\nFull stop outside parens shouldn't be included http://google.com/ok.\n\n(Full stop inside parens shouldn't be included http://google.com/ok.)\n\n\"http://google.com\"\n\n'http://google.com'\n\nhttp://🍄.ga/ http://x🍄.ga/\n",
            html: "<p>: <a href=\"http://google.com\">http://google.com</a> <a href=\"https://google.com\">https://google.com</a></p>\n<p><a href=\"http://google.com/%C3%A5\">http://google.com/å</a> <a href=\"http://google.com/%C3%A5\">http://google.com/å</a></p>\n<p><a href=\"mailto:scyther@pokemon.com\">scyther@pokemon.com</a></p>\n<p><a href=\"mailto:scy.the_rbe-edr+ill@pokemon.com\">scy.the_rbe-edr+ill@pokemon.com</a></p>\n<p><a href=\"mailto:scyther@pokemon.com\">scyther@pokemon.com</a>.</p>\n<p><a href=\"mailto:scyther@pokemon.com\">scyther@pokemon.com</a>/</p>\n<p><a href=\"mailto:scyther@pokemon.com\">scyther@pokemon.com</a>/<a href=\"mailto:beedrill@pokemon.com\">beedrill@pokemon.com</a></p>\n<p><a href=\"mailto:scyther@pokemon.com\">mailto:scyther@pokemon.com</a></p>\n<p>This is a <a href=\"mailto:scyther@pokemon.com\">mailto:scyther@pokemon.com</a></p>\n<p><a href=\"mailto:scyther@pokemon.com\">mailto:scyther@pokemon.com</a>.</p>\n<p>mmmmailto:<a href=\"mailto:scyther@pokemon.com\">scyther@pokemon.com</a></p>\n<p><a href=\"mailto:scyther@pokemon.com\">mailto:scyther@pokemon.com</a>/</p>\n<p><a href=\"mailto:scyther@pokemon.com\">mailto:scyther@pokemon.com</a>/message</p>\n<p><a href=\"mailto:scyther@pokemon.com\">mailto:scyther@pokemon.com</a>/<a href=\"mailto:beedrill@pokemon.com\">mailto:beedrill@pokemon.com</a></p>\n<p><a href=\"xmpp:scyther@pokemon.com\">xmpp:scyther@pokemon.com</a></p>\n<p><a href=\"xmpp:scyther@pokemon.com\">xmpp:scyther@pokemon.com</a>.</p>\n<p><a href=\"xmpp:scyther@pokemon.com/message\">xmpp:scyther@pokemon.com/message</a></p>\n<p><a href=\"xmpp:scyther@pokemon.com/message\">xmpp:scyther@pokemon.com/message</a>.</p>\n<p>Email me at:<a href=\"mailto:scyther@pokemon.com\">scyther@pokemon.com</a></p>\n<p><a href=\"http://www.github.com\">www.github.com</a> <a href=\"http://www.github.com/%C3%A1\">www.github.com/á</a></p>\n<p><a href=\"http://www.google.com/a_b\">www.google.com/a_b</a></p>\n<p>Underscores not allowed in host name www.xxx.yyy._zzz</p>\n<p>Underscores not allowed in host name www.xxx._yyy.zzz</p>\n<p>Underscores allowed in domain name <a href=\"http://www._xxx.yyy.zzz\">www._xxx.yyy.zzz</a></p>\n<p><strong>Autolink and <a href=\"http://inlines\">http://inlines</a></strong></p>\n<p><img src=\"http://inline.com/image\" alt=\"http://inline.com/image\" /></p>\n<p><a href=\"mailto:a.w@b.c\">a.w@b.c</a></p>\n<p>Full stop outside parens shouldn't be included <a href=\"http://google.com/ok\">http://google.com/ok</a>.</p>\n<p>(Full stop inside parens shouldn't be included <a href=\"http://google.com/ok\">http://google.com/ok</a>.)</p>\n<p>&quot;<a href=\"http://google.com\">http://google.com</a>&quot;</p>\n<p>'<a href=\"http://google.com\">http://google.com</a>'</p>\n<p><a href=\"http://%F0%9F%8D%84.ga/\">http://🍄.ga/</a> <a href=\"http://x%F0%9F%8D%84.ga/\">http://x🍄.ga/</a></p>\n",
        },
        Example {
            name: "ext autolinks not linked",
            markdown: "These should not link:\n\n* @a.b.c@. x\n* n@.  b\n",
            html: "<p>These should not link:</p>\n<ul>\n<li>@a.b.c@. x</li>\n<li>n@.  b</li>\n</ul>\n",
        },
        Example {
            name: "ext tagfilter",
            markdown: "This is <xmp> not okay, but **this** <strong>is</strong>.\n\n<p>This is <xmp> not okay, but **this** <strong>is</strong>.</p>\n\nNope, I won't have <textarea>.\n\n<p>No <textarea> here either.</p>\n\n<p>This <random /> <thing> is okay</thing> though.</p>\n\nYep, <totally>okay</totally>.\n\n<!-- HTML comments are okay, though. -->\n<!- But we're strict. ->\n<! No nonsense. >\n<!-- Leave multiline comments the heck alone, though, okay?\nEven with {\"x\":\"y\"} or 1 > 2 or whatever. Even **markdown**.\n-->\n<!--- Support everything CommonMark's parser does. -->\n<!---->\n<!--thistoo-->\n",
            html: "<p>This is &lt;xmp> not okay, but <strong>this</strong> <strong>is</strong>.</p>\n<p>This is &lt;xmp> not okay, but **this** <strong>is</strong>.</p>\n<p>Nope, I won't have &lt;textarea>.</p>\n<p>No &lt;textarea> here either.</p>\n<p>This <random /> <thing> is okay</thing> though.</p>\n<p>Yep, <totally>okay</totally>.</p>\n<!-- HTML comments are okay, though. -->\n<p>&lt;!- But we're strict. -&gt;\n&lt;! No nonsense. &gt;</p>\n<!-- Leave multiline comments the heck alone, though, okay?\nEven with {\"x\":\"y\"} or 1 > 2 or whatever. Even **markdown**.\n-->\n<!--- Support everything CommonMark's parser does. -->\n<!---->\n<!--thistoo-->\n",
        },
        Example {
            name: "ext footnotes",
            markdown: "This is some text![^1]. Other text.[^footnote].\n\nHere's a thing[^other-note].\n\nAnd another thing[^codeblock-note].\n\nThis doesn't have a referent[^nope].\n\n\n[^other-note]:       no code block here (spaces are stripped away)\n\n[^codeblock-note]:\n        this is now a code block (8 spaces indentation)\n\n[^1]: Some *bolded* footnote definition.\n\nHi!\n\n[^footnote]:\n    > Blockquotes can be in a footnote.\n\n        as well as code blocks\n\n    or, naturally, simple paragraphs.\n\n[^unused]: This is unused.\n",
            html: "<p>This is some text!<sup class=\"footnote-ref\"><a href=\"#fn-1\" id=\"fnref-1\" data-footnote-ref>1</a></sup>. Other text.<sup class=\"footnote-ref\"><a href=\"#fn-footnote\" id=\"fnref-footnote\" data-footnote-ref>2</a></sup>.</p>\n<p>Here's a thing<sup class=\"footnote-ref\"><a href=\"#fn-other-note\" id=\"fnref-other-note\" data-footnote-ref>3</a></sup>.</p>\n<p>And another thing<sup class=\"footnote-ref\"><a href=\"#fn-codeblock-note\" id=\"fnref-codeblock-note\" data-footnote-ref>4</a></sup>.</p>\n<p>This doesn't have a referent[^nope].</p>\n<p>Hi!</p>\n<section class=\"footnotes\" data-footnotes>\n<ol>\n<li id=\"fn-1\">\n<p>Some <em>bolded</em> footnote definition. <a href=\"#fnref-1\" class=\"footnote-backref\" data-footnote-backref data-footnote-backref-idx=\"1\" aria-label=\"Back to reference 1\">↩</a></p>\n</li>\n<li id=\"fn-footnote\">\n<blockquote>\n<p>Blockquotes can be in a footnote.</p>\n</blockquote>\n<pre><code>as well as code blocks\n</code></pre>\n<p>or, naturally, simple paragraphs. <a href=\"#fnref-footnote\" class=\"footnote-backref\" data-footnote-backref data-footnote-backref-idx=\"2\" aria-label=\"Back to reference 2\">↩</a></p>\n</li>\n<li id=\"fn-other-note\">\n<p>no code block here (spaces are stripped away) <a href=\"#fnref-other-note\" class=\"footnote-backref\" data-footnote-backref data-footnote-backref-idx=\"3\" aria-label=\"Back to reference 3\">↩</a></p>\n</li>\n<li id=\"fn-codeblock-note\">\n<pre><code>this is now a code block (8 spaces indentation)\n</code></pre>\n<a href=\"#fnref-codeblock-note\" class=\"footnote-backref\" data-footnote-backref data-footnote-backref-idx=\"4\" aria-label=\"Back to reference 4\">↩</a>\n</li>\n</ol>\n</section>\n",
        },
        Example {
            name: "ext footnotes multiple backrefs",
            markdown: "This is some text. It has a footnote[^a-footnote].\n\nThis footnote is referenced[^a-footnote] multiple times, in lots of different places.[^a-footnote]\n\n[^a-footnote]: This footnote definition should have three backrefs.\n",
            html: "<p>This is some text. It has a footnote<sup class=\"footnote-ref\"><a href=\"#fn-a-footnote\" id=\"fnref-a-footnote\" data-footnote-ref>1</a></sup>.</p>\n<p>This footnote is referenced<sup class=\"footnote-ref\"><a href=\"#fn-a-footnote\" id=\"fnref-a-footnote-2\" data-footnote-ref>1</a></sup> multiple times, in lots of different places.<sup class=\"footnote-ref\"><a href=\"#fn-a-footnote\" id=\"fnref-a-footnote-3\" data-footnote-ref>1</a></sup></p>\n<section class=\"footnotes\" data-footnotes>\n<ol>\n<li id=\"fn-a-footnote\">\n<p>This footnote definition should have three backrefs. <a href=\"#fnref-a-footnote\" class=\"footnote-backref\" data-footnote-backref data-footnote-backref-idx=\"1\" aria-label=\"Back to reference 1\">↩</a> <a href=\"#fnref-a-footnote-2\" class=\"footnote-backref\" data-footnote-backref data-footnote-backref-idx=\"1-2\" aria-label=\"Back to reference 1-2\">↩<sup class=\"footnote-ref\">2</sup></a> <a href=\"#fnref-a-footnote-3\" class=\"footnote-backref\" data-footnote-backref data-footnote-backref-idx=\"1-3\" aria-label=\"Back to reference 1-3\">↩<sup class=\"footnote-ref\">3</sup></a></p>\n</li>\n</ol>\n</section>\n",
        },
        Example {
            name: "ext footnote labels escaped",
            markdown: "Hello[^\"><script>alert(1)</script>]\n\n[^\"><script>alert(1)</script>]: pwned\n",
            html: "<p>Hello<sup class=\"footnote-ref\"><a href=\"#fn-%22%3E%3Cscript%3Ealert(1)%3C/script%3E\" id=\"fnref-%22%3E%3Cscript%3Ealert(1)%3C/script%3E\" data-footnote-ref>1</a></sup></p>\n<section class=\"footnotes\" data-footnotes>\n<ol>\n<li id=\"fn-%22%3E%3Cscript%3Ealert(1)%3C/script%3E\">\n<p>pwned <a href=\"#fnref-%22%3E%3Cscript%3Ealert(1)%3C/script%3E\" class=\"footnote-backref\" data-footnote-backref data-footnote-backref-idx=\"1\" aria-label=\"Back to reference 1\">↩</a></p>\n</li>\n</ol>\n</section>\n",
        },
        Example {
            name: "ext interop autolink strikethrough",
            markdown: "~~www.google.com~~\n\n~~http://google.com~~\n",
            html: "<p><del><a href=\"http://www.google.com\">www.google.com</a></del></p>\n<p><del><a href=\"http://google.com\">http://google.com</a></del></p>\n",
        },
        Example {
            name: "ext interop autolink table",
            markdown: "| a | b |\n| --- | --- |\n| https://github.com www.github.com | http://pokemon.com |\n",
            html: "<table>\n<thead>\n<tr>\n<th>a</th>\n<th>b</th>\n</tr>\n</thead>\n<tbody>\n<tr>\n<td><a href=\"https://github.com\">https://github.com</a> <a href=\"http://www.github.com\">www.github.com</a></td>\n<td><a href=\"http://pokemon.com\">http://pokemon.com</a></td>\n</tr>\n</tbody>\n</table>\n",
        },
        Example {
            name: "ext tasklist non-task markers",
            markdown: "- [x] foo\n  - [ ] bar\n  - [x] baz\n- [ ] bim\n\nShow a regular (non task) list to show that it has the same structure\n- [@] foo\n  - [@] bar\n  - [@] baz\n- [@] bim\n",
            html: "<ul>\n<li><input type=\"checkbox\" checked=\"\" disabled=\"\" /> foo\n<ul>\n<li><input type=\"checkbox\" disabled=\"\" /> bar</li>\n<li><input type=\"checkbox\" checked=\"\" disabled=\"\" /> baz</li>\n</ul>\n</li>\n<li><input type=\"checkbox\" disabled=\"\" /> bim</li>\n</ul>\n<p>Show a regular (non task) list to show that it has the same structure</p>\n<ul>\n<li>[@] foo\n<ul>\n<li>[@] bar</li>\n<li>[@] baz</li>\n</ul>\n</li>\n<li>[@] bim</li>\n</ul>\n",
        },
        Example {
            name: "ext tasklist wide indent",
            markdown: "- [x] foo\n    - [ ] bar\n    - [x] baz\n- [ ] bim\n",
            html: "<ul>\n<li><input type=\"checkbox\" checked=\"\" disabled=\"\" /> foo\n<ul>\n<li><input type=\"checkbox\" disabled=\"\" /> bar</li>\n<li><input type=\"checkbox\" checked=\"\" disabled=\"\" /> baz</li>\n</ul>\n</li>\n<li><input type=\"checkbox\" disabled=\"\" /> bim</li>\n</ul>\n",
        },
        Example {
            name: "comrak alert note",
            markdown: "> [!NOTE]\n> Highlights information that users should take into account, even when skimming.\n",
            html: "<div class=\"markdown-alert markdown-alert-note\">\n<p class=\"markdown-alert-title\">Note</p>\n<p>Highlights information that users should take into account, even when skimming.</p>\n</div>\n",
        },
        Example {
            name: "comrak alert hard break",
            markdown: "> [!NOTE]  \n> Highlights information that users should take into account, even when skimming.\n",
            html: "<div class=\"markdown-alert markdown-alert-note\">\n<p class=\"markdown-alert-title\">Note</p>\n<p>Highlights information that users should take into account, even when skimming.</p>\n</div>\n",
        },
        Example {
            name: "comrak alert multiple blocks",
            markdown: "> [!NOTE]\n> Highlights information that users should take into account, even when skimming.\n>\n> Paragraph two.\n",
            html: "<div class=\"markdown-alert markdown-alert-note\">\n<p class=\"markdown-alert-title\">Note</p>\n<p>Highlights information that users should take into account, even when skimming.</p>\n<p>Paragraph two.</p>\n</div>\n",
        },
        Example {
            name: "comrak alert caution",
            markdown: "> [!CAUTION]\n> Negative potential consequences of an action.\n",
            html: "<div class=\"markdown-alert markdown-alert-caution\">\n<p class=\"markdown-alert-title\">Caution</p>\n<p>Negative potential consequences of an action.</p>\n</div>\n",
        },
        Example {
            name: "comrak alert custom title",
            markdown: "> [!NOTE] **Pay** attention <script>\n> Highlights information that users should take into account, even when skimming.\n",
            html: "<div class=\"markdown-alert markdown-alert-note\">\n<p class=\"markdown-alert-title\">**Pay** attention &lt;script&gt;</p>\n<p>Highlights information that users should take into account, even when skimming.</p>\n</div>\n",
        },
        Example {
            name: "comrak alert in list",
            markdown: "- Item one\n\n  > [!NOTE]\n  > Highlights information that users should take into account, even when skimming.\n",
            html: "<ul>\n<li>\n<p>Item one</p>\n<div class=\"markdown-alert markdown-alert-note\">\n<p class=\"markdown-alert-title\">Note</p>\n<p>Highlights information that users should take into account, even when skimming.</p>\n</div>\n</li>\n</ul>\n",
        },
    ];
}

/// Renders a document with marki and with comrak's GFM configuration.
struct Renderers;

impl Renderers {
    fn marki(markdown: &str) -> String {
        let normalized = MarkdownFile::normalize(markdown);
        let md: MarkdownFile<'_> = MarkdownFile::parse(&normalized);
        md.to_html()
    }

    fn comrak(markdown: &str) -> String {
        let mut options = comrak::Options::default();
        options.extension.strikethrough = true;
        options.extension.table = true;
        options.extension.autolink = true;
        options.extension.tasklist = true;
        options.extension.tagfilter = true;
        options.extension.footnotes = true;
        options.extension.alerts = true;
        options.render.r#unsafe = true;
        comrak::markdown_to_html(markdown, &options)
    }
}

/// Runs a set of `(name, markdown, expected)` cases and reports every failure.
struct Report {
    failures: Vec<String>,
    total: usize,
}

impl Report {
    fn run<'a>(
        cases: impl IntoIterator<Item = (&'a str, &'a str, String)>,
        render: impl Fn(&str) -> String,
    ) -> Self {
        let mut failures = Vec::new();
        let mut total = 0;
        for (name, markdown, expected) in cases {
            total += 1;
            let got = render(markdown);
            if got != expected {
                failures.push(format!(
                    "{name}:\n  input:    {markdown:?}\n  expected: {expected:?}\n  got:      {got:?}"
                ));
            }
        }
        Self { failures, total }
    }

    fn assert_clean(&self, label: &str) {
        for failure in &self.failures {
            eprintln!("\nFAIL {failure}");
        }
        assert!(
            self.failures.is_empty(),
            "{label}: {}/{} examples failed",
            self.failures.len(),
            self.total
        );
    }
}

#[test]
fn gfm_extension_examples() {
    Report::run(
        Corpus::EXAMPLES
            .iter()
            .map(|e| (e.name, e.markdown, e.html.to_owned())),
        Renderers::marki,
    )
    .assert_clean("GFM extension examples");
}

#[test]
fn comrak_agrees_with_fixture() {
    Report::run(
        Corpus::EXAMPLES
            .iter()
            .filter(|e| e.name != "ext autolinks")
            .map(|e| (e.name, e.markdown, e.html.to_owned())),
        Renderers::comrak,
    )
    .assert_clean("comrak on the GFM fixture");
}

#[test]
fn matches_comrak_on_mixed_documents() {
    let documents: &[(&str, &str)] = &[
        (
            "everything",
            "# Notes\n\n- [x] ship ~~0.2~~ 0.3\n- [ ] see www.example.com/docs and mail a@b.io\n\n\
| col | val |\n|:--|--:|\n| a | **1** |\n\n> [!TIP]\n> Read the footnote[^n].\n\n\
<iframe src=x></iframe>\n\n[^n]: It explains *why*.\n",
        ),
        (
            "nested containers",
            "> - [x] quoted task\n>   with a lazy line\n\n1. item\n   > [!WARNING] Careful\n   > body\n",
        ),
        (
            "footnote order",
            "b[^b] a[^a] b again[^b]\n\n[^a]: first\n\n[^b]: second\n    continued\n",
        ),
        (
            "footnote with lazy line and list",
            "x[^1]\n\n[^1]: para\nlazy line\n\n    - item\n    - [x] task\n\nafter\n",
        ),
        (
            "autolinks around syntax",
            "*www.a.com* _http://b.org/x_ (www.c.net) [x www.d.io] e@f.gh.\n",
        ),
        (
            "strike and emphasis",
            "~~**a**~~ ~b~ ~~~d~~~ *~~e~~*\n\n~~c~ and ~x ~~y~\n",
        ),
        (
            "tables with inline gfm",
            "| a | b |\n|---|---|\n| ~~x~~ | www.y.com |\n| [^n] | `c\\|d` |\n\n[^n]: note\n",
        ),
        (
            "alert kinds",
            "> [!important]\n> one\n\n> [!Warning] Title here\n> two\n\n> [!unknown]\n> three\n",
        ),
    ];
    Report::run(
        documents
            .iter()
            .map(|&(name, md)| (name, md, Renderers::comrak(md))),
        Renderers::marki,
    )
    .assert_clean("marki vs comrak");
}
