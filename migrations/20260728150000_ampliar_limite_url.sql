-- Raises the long_url ceiling from 2048 to 8192 characters.
--
-- 2048 was justified as "the practical browser limit". That number is folklore
-- inherited from Internet Explorer's 2083, and it does not apply here twice
-- over: the long URL travels in a POST body rather than an address bar, and
-- current browsers accept far longer ones.
--
-- Nothing downstream was actually pressed by it. Cloudflare allows 128 KB of
-- headers each way, so the `Location` of the redirect was never close. The
-- `url_hash` index is a fixed 32 bytes, which is exactly why the unique
-- constraint was moved onto a hash in the first place.
--
-- 8192 is where a real limit begins: nginx serves ~8 KB of request line plus
-- headers by default and answers 414 above that. Storing a longer URL would
-- mean storing a link that does not work at its destination.
--
-- Cheap now, expensive later: ADD CONSTRAINT revalidates every existing row.
-- At the projected scale this would need NOT VALID plus a separate VALIDATE.

ALTER TABLE urls DROP CONSTRAINT urls_long_url_check;

ALTER TABLE urls ADD CONSTRAINT urls_long_url_check CHECK (length(long_url) <= 8192);
