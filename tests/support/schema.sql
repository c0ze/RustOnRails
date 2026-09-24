
SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;
SELECT pg_catalog.set_config('search_path', '', false);
SET check_function_bodies = false;
SET xmloption = content;
SET client_min_messages = warning;
SET row_security = off;

SET default_tablespace = '';

SET default_table_access_method = heap;

CREATE TABLE public.comments (
    id bigint NOT NULL,
    post_id bigint NOT NULL,
    user_id bigint NOT NULL,
    body text NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);

CREATE SEQUENCE public.comments_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.comments_id_seq OWNED BY public.comments.id;

CREATE TABLE public.posts (
    id bigint NOT NULL,
    user_id bigint NOT NULL,
    title character varying NOT NULL,
    body text,
    status integer DEFAULT 0 NOT NULL,
    published_at timestamp(6) without time zone,
    comments_count integer DEFAULT 0 NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);

CREATE SEQUENCE public.posts_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.posts_id_seq OWNED BY public.posts.id;

CREATE TABLE public.users (
    id bigint NOT NULL,
    name character varying NOT NULL,
    email character varying NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL
);

CREATE SEQUENCE public.users_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.users_id_seq OWNED BY public.users.id;

ALTER TABLE ONLY public.comments ALTER COLUMN id SET DEFAULT nextval('public.comments_id_seq'::regclass);

ALTER TABLE ONLY public.posts ALTER COLUMN id SET DEFAULT nextval('public.posts_id_seq'::regclass);

ALTER TABLE ONLY public.users ALTER COLUMN id SET DEFAULT nextval('public.users_id_seq'::regclass);

ALTER TABLE ONLY public.comments
    ADD CONSTRAINT comments_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.posts
    ADD CONSTRAINT posts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_pkey PRIMARY KEY (id);

CREATE INDEX index_comments_on_post_id ON public.comments USING btree (post_id);

CREATE INDEX index_comments_on_user_id ON public.comments USING btree (user_id);

CREATE INDEX index_posts_on_user_id ON public.posts USING btree (user_id);

CREATE UNIQUE INDEX index_users_on_email ON public.users USING btree (email);

ALTER TABLE ONLY public.comments
    ADD CONSTRAINT fk_rails_03de2dc08c FOREIGN KEY (user_id) REFERENCES public.users(id);

ALTER TABLE ONLY public.comments
    ADD CONSTRAINT fk_rails_2fd19c0db7 FOREIGN KEY (post_id) REFERENCES public.posts(id);

ALTER TABLE ONLY public.posts
    ADD CONSTRAINT fk_rails_5b5ddfd518 FOREIGN KEY (user_id) REFERENCES public.users(id);

